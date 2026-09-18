mod commands;
mod welcome;

// Android-only JNI bridge: exposes grafium_core::assistant::handle_command as
// `Java_com_grafium_app_AssistantReceiver_nativeHandleCommand` so the Kotlin
// receiver can share the same NLU as the desktop Tauri command above.
#[cfg(target_os = "android")]
mod android_jni;

use commands::graph::GraphConfig;
use grafium_core::Graph;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};
use welcome::seed_tutorial_graph;

pub struct AppState {
    pub graph: Arc<Mutex<Graph>>,
    watcher: Mutex<Option<GraphWatcherHandle>>,
}

#[tauri::command(rename_all = "camelCase")]
fn debug_log(message: String) {
    tracing::debug!("{}", message);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GraphRuntimeSnapshot {
    pub root_dir: PathBuf,
    pub db_path: PathBuf,
    pub metadata_dir_name: String,
}

struct GraphWatcherHandle {
    stop_tx: mpsc::Sender<()>,
    join_handle: thread::JoinHandle<()>,
}

impl GraphWatcherHandle {
    fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join_handle.join();
    }
}

impl AppState {
    pub fn restart_graph_watcher(&self) -> Result<(), String> {
        if let Ok(mut guard) = self.watcher.lock() {
            if let Some(existing) = guard.take() {
                existing.stop();
            }
        }

        let (pages_dir, journals_dir, knowledge_dir, self_writes) = {
            let graph = self.graph.lock().map_err(|e| e.to_string())?;
            (
                graph.pages_dir.clone(),
                graph.journals_dir.clone(),
                graph.knowledge_dir.clone(),
                graph.self_write_tracker(),
            )
        };

        let handle = start_graph_watcher(
            self.graph.clone(),
            pages_dir,
            journals_dir,
            knowledge_dir,
            self_writes,
        )?;
        let mut guard = self.watcher.lock().map_err(|e| e.to_string())?;
        *guard = Some(handle);
        Ok(())
    }
}

pub(crate) fn snapshot_then<T, S, R>(
    lock: &Mutex<T>,
    snapshot: impl FnOnce(&T) -> Result<S, String>,
    run: impl FnOnce(S) -> Result<R, String>,
) -> Result<R, String> {
    let snapshot = {
        let guard = lock.lock().map_err(|e| e.to_string())?;
        snapshot(&guard)?
    };
    run(snapshot)
}

pub(crate) fn current_graph_snapshot(
    app: &tauri::AppHandle,
    graph: &Mutex<Graph>,
) -> Result<GraphRuntimeSnapshot, String> {
    snapshot_then(
        graph,
        |graph| {
            Ok(GraphRuntimeSnapshot {
                root_dir: graph.root_dir.clone(),
                db_path: platform_db_path(app, &graph.root_dir),
                metadata_dir_name: metadata_dir_name(app),
            })
        },
        |snapshot| Ok(snapshot),
    )
}

pub(crate) fn open_graph_snapshot(snapshot: &GraphRuntimeSnapshot) -> Result<Graph, String> {
    Graph::open_with_db_path_and_metadata_dir(
        &snapshot.root_dir,
        &snapshot.db_path,
        &snapshot.metadata_dir_name,
    )
    .map_err(|e| e.to_string())
}

fn should_process_event(
    event: &Event,
    pages_dir: &std::path::Path,
    journals_dir: &std::path::Path,
    knowledge_dir: &std::path::Path,
) -> bool {
    event.paths.iter().any(|p| {
        p.extension().and_then(|e| e.to_str()) == Some("md")
            && (p.starts_with(pages_dir)
                || p.starts_with(journals_dir)
                || p.starts_with(knowledge_dir))
    })
}

/// Returns true if `path` was written by the app itself within the last few
/// seconds. Used to ignore self-inflicted filesystem events so a normal block
/// save doesn't get mistaken for an external edit.
fn was_recent_self_write(self_writes: &Arc<Mutex<HashMap<PathBuf, Instant>>>, path: &Path) -> bool {
    if let Ok(mut map) = self_writes.lock() {
        let now = Instant::now();
        map.retain(|_, t| now.duration_since(*t).as_secs() < 30);
        if let Some(t) = map.get(path) {
            return now.duration_since(*t).as_secs() < 10;
        }
    }
    false
}

fn start_graph_watcher(
    graph: Arc<Mutex<Graph>>,
    pages_dir: PathBuf,
    journals_dir: PathBuf,
    knowledge_dir: PathBuf,
    self_writes: Arc<Mutex<HashMap<PathBuf, Instant>>>,
) -> Result<GraphWatcherHandle, String> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (event_tx, event_rx) = mpsc::channel::<notify::Result<Event>>();

    let join_handle = thread::spawn(move || {
        let mut watcher = match RecommendedWatcher::new(
            move |res| {
                let _ = event_tx.send(res);
            },
            Config::default(),
        ) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("watcher init failed: {}", e);
                return;
            }
        };

        if let Err(e) = watcher.watch(&pages_dir, RecursiveMode::Recursive) {
            eprintln!("watch pages dir failed: {}", e);
            return;
        }
        if let Err(e) = watcher.watch(&journals_dir, RecursiveMode::Recursive) {
            eprintln!("watch journals dir failed: {}", e);
            return;
        }
        if let Err(e) = watcher.watch(&knowledge_dir, RecursiveMode::Recursive) {
            eprintln!("watch knowledge dir failed: {}", e);
            return;
        }

        let debounce = Duration::from_millis(400);
        let mut pending_files = std::collections::HashSet::<PathBuf>::new();
        let mut last_event_at: Option<Instant> = None;

        loop {
            if stop_rx.try_recv().is_ok() {
                break;
            }

            match event_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(event)) => {
                    if !should_process_event(&event, &pages_dir, &journals_dir, &knowledge_dir) {
                        continue;
                    }

                    for path in event.paths {
                        if path.extension().and_then(|e| e.to_str()) != Some("md") {
                            continue;
                        }
                        // Ignore writes the app just made itself. Without this,
                        // every block save (which rewrites the page's .md file)
                        // would be re-processed as an external change.
                        if was_recent_self_write(&self_writes, &path) {
                            continue;
                        }
                        pending_files.insert(path);
                    }

                    if !pending_files.is_empty() {
                        last_event_at = Some(Instant::now());
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("watch event error: {}", e);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            let ready = last_event_at
                .map(|t| t.elapsed() >= debounce)
                .unwrap_or(false);

            if !ready {
                continue;
            }

            let g = match graph.lock() {
                Ok(guard) => guard,
                Err(e) => {
                    eprintln!("watch lock error: {}", e);
                    pending_files.clear();
                    last_event_at = None;
                    continue;
                }
            };

            // Incremental indexing only. We deliberately NEVER call
            // `reindex_all()` from the watcher: it runs `clear_all()` (wiping
            // the entire index) followed by a full disk rescan. On a large
            // graph that both freezes the app for a long time and — if the
            // on-disk .md files are not a complete mirror of the index — can
            // destroy data. Index only the changed files. Deletions are left
            // alone (a stale entry is harmless; an explicit re-index fixes it).
            for path in pending_files.drain() {
                if path.exists() {
                    if let Err(e) = g.index_file(&path) {
                        eprintln!("watch index file failed ({}): {}", path.display(), e);
                    }
                }
            }

            drop(g);
            pending_files.clear();
            last_event_at = None;
        }
    });

    Ok(GraphWatcherHandle {
        stop_tx,
        join_handle,
    })
}

/// Watch ~/.config/smplos/current/theme.name for changes and emit event to frontend
fn start_smplos_theme_watcher(app_handle: tauri::AppHandle) {
    let theme_path = match dirs::config_dir() {
        Some(d) => d.join("smplos/current/theme.name"),
        None => return,
    };

    if !theme_path.exists() {
        return;
    }

    thread::spawn(move || {
        use std::fs;

        let mut last_content = fs::read_to_string(&theme_path)
            .unwrap_or_default()
            .trim()
            .to_string();

        loop {
            thread::sleep(Duration::from_secs(2));

            let current = match fs::read_to_string(&theme_path) {
                Ok(s) => s.trim().to_string(),
                Err(_) => continue,
            };

            if current != last_content && !current.is_empty() {
                eprintln!(
                    "[theme-watcher] smplos theme changed: {} -> {}",
                    last_content, current
                );
                last_content = current.clone();
                let _ = app_handle.emit(
                    "smplos-theme-changed",
                    serde_json::json!({
                        "theme": current,
                    }),
                );
            }
        }
    });
}

/// Background thread that periodically checks if sync targets become available.
/// When a target that was unavailable becomes available, it emits a Tauri event
/// and optionally triggers auto-sync.
fn start_sync_monitor(app_handle: tauri::AppHandle, graph: Arc<Mutex<Graph>>) {
    use grafium_core::sync::{
        filesystem::FilesystemBackend,
        state::{BackendConfig, SyncConfigs},
        webdav::WebDavBackend,
        SyncEngine,
    };

    thread::spawn(move || {
        let mut was_available: std::collections::HashMap<String, bool> =
            std::collections::HashMap::new();
        let check_interval = Duration::from_secs(5);

        loop {
            thread::sleep(check_interval);

            let snapshot = match current_graph_snapshot(&app_handle, graph.as_ref()) {
                Ok(snapshot) => snapshot,
                Err(_) => continue,
            };

            let config_path = snapshot
                .root_dir
                .join(&snapshot.metadata_dir_name)
                .join("sync-config.json");
            let configs = SyncConfigs::load(&config_path);

            for target in &configs.targets {
                let backend: Box<dyn grafium_core::sync::SyncBackend> = match &target.config {
                    BackendConfig::Filesystem { path } => {
                        Box::new(FilesystemBackend::new(path.clone(), target.name.clone()))
                    }
                    BackendConfig::WebDav {
                        url,
                        username,
                        password,
                    } => match WebDavBackend::new(
                        url.clone(),
                        username.clone(),
                        password.clone(),
                        target.name.clone(),
                    ) {
                        Ok(backend) => Box::new(backend),
                        Err(err) => {
                            eprintln!(
                                "[sync-monitor] Skipping target '{}' because WebDAV backend initialization failed: {}",
                                target.name,
                                err
                            );
                            continue;
                        }
                    },
                };

                let now_available = backend.is_available();
                let previously_available = was_available.get(&target.id).copied().unwrap_or(false);

                if now_available && !previously_available {
                    // Target just became available!
                    tracing::info!("Target '{}' is now available", target.name);

                    // Emit event to frontend
                    let _ = app_handle.emit(
                        "sync-target-available",
                        serde_json::json!({
                            "target_id": target.id,
                            "target_name": target.name,
                        }),
                    );

                    // Auto-sync if enabled
                    if target.auto_sync {
                        let engine = SyncEngine::new_with_metadata_dir(
                            snapshot.root_dir.clone(),
                            &snapshot.metadata_dir_name,
                        );
                        match engine.sync(backend.as_ref()) {
                            Ok(result) => {
                                eprintln!(
                                    "[sync-monitor] Auto-sync '{}': {}",
                                    target.name,
                                    result.summary()
                                );

                                // Reindex if we pulled files
                                if !result.pulled.is_empty()
                                    || !result.conflicts.is_empty()
                                    || !result.deleted_local.is_empty()
                                {
                                    if let Ok(detached_graph) = open_graph_snapshot(&snapshot) {
                                        let _ = detached_graph.reindex_all();
                                    }
                                    // Notify frontend to refresh
                                    let _ = app_handle.emit(
                                        "sync-completed",
                                        serde_json::json!({
                                            "target_name": target.name,
                                            "pushed": result.pushed.len(),
                                            "pulled": result.pulled.len(),
                                            "conflicts": result.conflicts.len(),
                                        }),
                                    );
                                }
                            }
                            Err(e) => {
                                eprintln!(
                                    "[sync-monitor] Auto-sync '{}' failed: {}",
                                    target.name, e
                                );
                                let _ = app_handle.emit(
                                    "sync-error",
                                    serde_json::json!({
                                        "target_name": target.name,
                                        "error": e.to_string(),
                                    }),
                                );
                            }
                        }
                    }
                } else if !now_available && previously_available {
                    // Target disconnected
                    let _ = app_handle.emit(
                        "sync-target-disconnected",
                        serde_json::json!({
                            "target_id": target.id,
                            "target_name": target.name,
                        }),
                    );
                }

                was_available.insert(target.id.clone(), now_available);
            }
        }
    });
}

/// Debounce window: a page must be quiescent (no further edits) for this long
/// after its last edit before it's reindexed, so typing never triggers
/// embedding and rapid edits coalesce into a single reindex.
const REINDEX_DEBOUNCE_MS: i64 = 15_000;
/// How often the drainer wakes to look for quiesced pages.
const REINDEX_CYCLE: Duration = Duration::from_secs(5);
/// Gentle startup delay so we don't hammer the embedder the instant the app
/// opens while the user is trying to read something.
const REINDEX_STARTUP_DELAY: Duration = Duration::from_secs(20);
/// Cap pages reindexed per cycle so a crash-recovered backlog drains gradually
/// rather than saturating the embedder worker in one burst.
const REINDEX_MAX_PER_CYCLE: i64 = 8;

/// Background drainer that keeps the vector (semantic) index fresh as the user
/// writes, edits, imports, deletes, and moves blocks — no manual "Index my
/// notes" click after the first one.
///
/// Design:
/// - Work is discovered from the persisted `pending_reindex` table (marked at
///   the `Graph` write choke points), so edits made while the app was closed
///   are still picked up on the next launch (crash-safe).
/// - Per-page debounce + coalescing: a page is only processed once it's been
///   quiet for [`REINDEX_DEBOUNCE_MS`]; repeated edits keep bumping its
///   `marked_at`, collapsing N edits into one reindex.
/// - Page-level granularity: an ancestor edit changes descendants' breadcrumbs,
///   so the whole page is re-chunked — but the content-hash diff means only
///   genuinely-changed chunks are re-embedded.
/// - Degrades to a silent no-op when AI isn't fully configured (no embedder /
///   engine not ready): never an error toast, never a retry storm.
/// - Deletions: a pending id that no longer resolves to a page is treated as a
///   removal and its vectors are purged, so Chat never cites a deleted page.
fn start_reindex_drainer(
    app_handle: tauri::AppHandle,
    graph: Arc<Mutex<Graph>>,
    engine: Arc<tokio::sync::RwLock<Option<grafium_core::KnowledgeEngine>>>,
) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(REINDEX_STARTUP_DELAY).await;

        loop {
            tokio::time::sleep(REINDEX_CYCLE).await;

            // No-op unless AI is fully ready. Pending rows wait harmlessly
            // until an embedder is configured, then get picked up.
            let ready = {
                let guard = engine.read().await;
                guard.as_ref().map(|e| e.can_index()).unwrap_or(false)
            };
            if !ready {
                continue;
            }

            // Snapshot the due set + graph id under the sync lock, then drop it
            // before any await — holding a std Mutex across .await is unsound.
            let snapshot = {
                let g = match graph.lock() {
                    Ok(g) => g,
                    Err(_) => continue,
                };
                let graph_id = g.root_dir.to_string_lossy().to_string();
                match g
                    .db
                    .list_pending_reindex_due(REINDEX_DEBOUNCE_MS, REINDEX_MAX_PER_CYCLE)
                {
                    Ok(due) => Some((graph_id, due)),
                    Err(e) => {
                        eprintln!("reindex drainer: could not list pending pages: {e}");
                        None
                    }
                }
            };
            let (graph_id, due) = match snapshot {
                Some((graph_id, due)) if !due.is_empty() => (graph_id, due),
                _ => continue,
            };

            // Restore the hash cache once so a fresh process re-embeds only
            // genuinely-changed chunks, not whole pages, after a restart.
            {
                let guard = engine.read().await;
                if let Some(e) = guard.as_ref() {
                    if let Err(err) = e.restore_hash_cache(&graph_id).await {
                        eprintln!("reindex drainer: hash cache restore failed: {err}");
                    }
                }
            }

            let mut changed = false;
            for (page_id, marked_at) in due {
                // Load the page + its blocks under the sync lock, then drop it.
                // A page that no longer exists is a deletion → purge vectors.
                enum Work {
                    Reindex(grafium_core::models::Page, Vec<grafium_core::models::Block>),
                    Remove,
                    Skip,
                }
                let work = {
                    let g = match graph.lock() {
                        Ok(g) => g,
                        Err(_) => break,
                    };
                    match g.db.get_page_by_id(&page_id) {
                        Ok(page) => match g.db.list_blocks_for_page(&page_id) {
                            Ok(blocks) => Work::Reindex(page, blocks),
                            Err(e) => {
                                eprintln!(
                                    "reindex drainer: list blocks failed for '{page_id}': {e}"
                                );
                                Work::Skip
                            }
                        },
                        Err(_) => Work::Remove,
                    }
                };

                let result = {
                    let guard = engine.read().await;
                    let e = match guard.as_ref() {
                        Some(e) => e,
                        None => break,
                    };
                    match &work {
                        Work::Reindex(page, blocks) => {
                            e.index_page(page, blocks, &graph_id).await.map(|_| ())
                        }
                        Work::Remove => e.remove_page(&graph_id, &page_id).await,
                        Work::Skip => continue,
                    }
                };

                match result {
                    Ok(()) => {
                        // Clear only if no newer edit bumped marked_at while we
                        // were embedding; otherwise leave it for the next cycle.
                        if let Ok(g) = graph.lock() {
                            let _ = g.db.clear_pending_reindex(&page_id, marked_at);
                        }
                        changed = true;
                    }
                    Err(e) => {
                        // Leave the row pending; the next cycle retries. Silent.
                        eprintln!("reindex drainer: reindex of '{page_id}' failed: {e}");
                    }
                }
            }

            if changed {
                // Nudge the UI to refresh its coverage / "N pages pending".
                let _ = app_handle.emit("ai-index-updated", ());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::snapshot_then;

    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn snapshot_then_releases_lock_before_running_work() {
        let shared = Arc::new(Mutex::new(41usize));
        let worker_shared = Arc::clone(&shared);
        let (started_tx, started_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();

        let worker = thread::spawn(move || {
            snapshot_then(
                worker_shared.as_ref(),
                |value| Ok(*value + 1),
                |snapshot| {
                    started_tx.send(snapshot).unwrap();
                    finish_rx.recv_timeout(Duration::from_secs(1)).unwrap();
                    Ok(())
                },
            )
            .unwrap();
        });

        assert_eq!(started_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 42);
        assert!(
            shared.try_lock().is_ok(),
            "expensive work should not run while holding the shared graph mutex"
        );

        finish_tx.send(()).unwrap();
        worker.join().unwrap();
    }
}

#[cfg(target_os = "android")]
fn stable_path_id(path: &std::path::Path) -> String {
    // Deterministic FNV-1a hash for filesystem-safe DB directory names.
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in path.to_string_lossy().as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

fn metadata_dir_name(app: &tauri::AppHandle) -> String {
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

fn platform_db_path(app: &tauri::AppHandle, graph_root: &std::path::Path) -> PathBuf {
    #[cfg(target_os = "android")]
    {
        let app_data = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("/data/local/tmp"));
        let id = stable_path_id(graph_root);
        return app_data.join("graph_indexes").join(id).join("index.db");
    }

    #[cfg(not(target_os = "android"))]
    {
        graph_root.join(metadata_dir_name(app)).join("index.db")
    }
}

/// Serves local graph assets (images, audio, video) to the webview through the
/// custom `grafium-asset://localhost/<relative-path>` scheme.
///
/// The path is resolved against the active graph's root directory. Requests are
/// confined to that directory (path-traversal attempts are rejected) so the
/// scheme can only read files inside the current graph.
fn asset_scheme_handler(
    app: &tauri::AppHandle,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::{Response, StatusCode};

    let not_found = || {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Vec::new())
            .unwrap()
    };

    // Extract and percent-decode the request path (strip leading '/').
    let raw_path = request.uri().path().trim_start_matches('/');
    let decoded = match urlencoding::decode(raw_path) {
        Ok(d) => d.into_owned(),
        Err(_) => return not_found(),
    };

    // Reject absolute paths and any traversal component up-front.
    if decoded.is_empty() || decoded.starts_with('/') || decoded.split('/').any(|c| c == "..") {
        return not_found();
    }

    // Resolve against the active graph root.
    let root = match app.try_state::<AppState>() {
        Some(state) => match state.graph.lock() {
            Ok(g) => g.root_dir.clone(),
            Err(_) => return not_found(),
        },
        None => return not_found(),
    };

    // Falls back to the shared `assets/` folder when a page-relative reference
    // misses, so notes written before media co-location still render.
    let canon_target = match grafium_core::graph::resolve_asset_path(&root, &decoded) {
        Some(p) => p,
        None => return not_found(),
    };

    let bytes = match std::fs::read(&canon_target) {
        Ok(b) => b,
        Err(_) => return not_found(),
    };

    let mime = mime_for_path(&canon_target);
    let total = bytes.len() as u64;

    // WebKitGTK's media backend requires Range support for <audio>/<video>
    // playback — without it the element fails to load ("error"). Honor a single
    // byte-range request and always advertise Accept-Ranges.
    let range = request
        .headers()
        .get("range")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| parse_byte_range(h, total));

    if let Some((start, end)) = range {
        let slice = bytes[start as usize..=end as usize].to_vec();
        return Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header("Content-Type", mime)
            .header("Accept-Ranges", "bytes")
            .header("Content-Range", format!("bytes {start}-{end}/{total}"))
            .header("Content-Length", (end - start + 1).to_string())
            .header("Access-Control-Allow-Origin", "*")
            .header("Cache-Control", "public, max-age=31536000, immutable")
            .body(slice)
            .unwrap_or_else(|_| not_found());
    }

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", mime)
        .header("Accept-Ranges", "bytes")
        .header("Content-Length", total.to_string())
        .header("Access-Control-Allow-Origin", "*")
        .header("Cache-Control", "public, max-age=31536000, immutable")
        .body(bytes)
        .unwrap_or_else(|_| not_found())
}

/// Parse a single `Range: bytes=START-END` header into an inclusive, clamped
/// (start, end) pair. Supports open-ended (`bytes=500-`) and suffix
/// (`bytes=-500`) forms. Returns None for an empty file or an unsatisfiable range.
fn parse_byte_range(header: &str, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    let spec = header.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (s, e) = spec.split_once('-')?;
    let (start, end) = if s.is_empty() {
        // Suffix range: the last N bytes.
        let n: u64 = e.parse().ok()?;
        if n == 0 {
            return None;
        }
        (total.saturating_sub(n), total - 1)
    } else {
        let start: u64 = s.parse().ok()?;
        let end: u64 = if e.is_empty() {
            total - 1
        } else {
            e.parse::<u64>().ok()?.min(total - 1)
        };
        (start, end)
    };
    if start > end || start >= total {
        return None;
    }
    Some((start, end))
}

/// Best-effort MIME type from a file extension for the asset scheme.
pub(crate) fn mime_for_path(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        // Audio
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        // Video
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "ogv" => "video/ogg",
        _ => "application/octet-stream",
    }
}

/// A `tracing_subscriber::Layer` that forwards every event into
/// [`grafium_core::log_tap`], so the media / AI code paths can read
/// back the actual whisper.cpp / GGML / llama.cpp status lines after a
/// load or generation and surface them to the user (e.g. "Vulkan not
/// available, falling back to CPU"). Runs alongside the existing
/// `fmt` layer, so developer stderr logging is unaffected.
struct LogTapLayer;

impl<S> tracing_subscriber::Layer<S> for LogTapLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        use grafium_core::log_tap::{record, TapLevel};

        struct MessageVisitor {
            message: String,
        }
        impl tracing::field::Visit for MessageVisitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                // The conventional message field is named "message";
                // other fields get appended as key=value so the tap
                // still captures useful context (e.g. `n_ctx=8192`).
                if field.name() == "message" {
                    self.message = format!("{value:?}");
                } else if !self.message.is_empty() {
                    use std::fmt::Write;
                    let _ = write!(&mut self.message, " {}={:?}", field.name(), value);
                } else {
                    self.message = format!("{}={:?}", field.name(), value);
                }
            }
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "message" {
                    self.message = value.to_string();
                } else if !self.message.is_empty() {
                    use std::fmt::Write;
                    let _ = write!(&mut self.message, " {}={}", field.name(), value);
                } else {
                    self.message = format!("{}={}", field.name(), value);
                }
            }
        }

        let mut visitor = MessageVisitor {
            message: String::new(),
        };
        event.record(&mut visitor);

        let level = match *event.metadata().level() {
            tracing::Level::TRACE => TapLevel::Trace,
            tracing::Level::DEBUG => TapLevel::Debug,
            tracing::Level::INFO => TapLevel::Info,
            tracing::Level::WARN => TapLevel::Warn,
            tracing::Level::ERROR => TapLevel::Error,
        };

        record(level, event.metadata().target(), &visitor.message);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Without this, every `tracing::info!`/`tracing::warn!` call throughout
    // the codebase (including grafium-core) silently went nowhere -- a real
    // observability gap that hampered debugging the OOM-crash investigation.
    // `RUST_LOG` still overrides the default if set; otherwise `info` is a
    // reasonable default for a desktop app (not so verbose it drowns out
    // the signal, but enough to see lifecycle/AI-provider/page-load events).
    //
    // The `log_tap` layer runs *alongside* the fmt layer so every event
    // continues to hit stderr for developer diagnostics *and* is captured
    // in an in-memory ring buffer that
    // `media::transcribe` / `ai::providers::local_llm` can read back after
    // a load / generation call, so they can surface the actual whisper.cpp
    // / GGML / llama.cpp status lines (e.g. "no Vulkan device found",
    // "using Vulkan backend") to the user in the progress UI instead of
    // leaving them staring at a silent dialog.
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(LogTapLayer)
        .init();

    // WebKitGTK on Wayland aborts with "Error 71 (Protocol error)" on some
    // GPU/compositor setups when the DMABUF renderer / accelerated compositing
    // is active. Disable them before the webview initializes so the app launches
    // reliably from any entry point (start menu, terminal, packaged binary)
    // without depending on an external wrapper script to set these.
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        }
    }

    tauri::Builder::default()
        .manage(commands::startup::StartupWindow::default())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .register_uri_scheme_protocol("grafium-asset", |ctx, request| {
            asset_scheme_handler(ctx.app_handle(), request)
        })
        .setup(|app| {
            #[cfg(desktop)]
            commands::startup::install_fallback(app.handle());
            let app_dir = app.path().app_data_dir().expect("Failed to get app data dir");
            let config_path = app_dir.join("graphs.json");
            let config = GraphConfig::load(&config_path)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

            // Use last-used graph, or fall back to default graph dir.
            // Validate the saved path before trusting it — if it no longer has a proper
            // graph structure, fall back to the default directory rather than creating
            // subdirectories inside an arbitrary user folder.
            let default_graph_dir = app_dir.join("tutorial-graph");
            let graph_dir = if let Some(ref current) = config.current {
                let candidate = PathBuf::from(current);
                let validation = Graph::validate_structure_with_metadata_dir(
                    &candidate,
                    &metadata_dir_name(app.handle()),
                );
                if validation.is_valid {
                    candidate
                } else {
                    eprintln!(
                        "Warning: saved graph path '{}' is no longer valid ({}), falling back to default",
                        current,
                        validation.error_message.as_deref().unwrap_or("unknown error")
                    );
                    default_graph_dir.clone()
                }
            } else {
                default_graph_dir.clone()
            };

            let db_path = platform_db_path(app.handle(), &graph_dir);
            let metadata_dir = metadata_dir_name(app.handle());
            let graph = Graph::open_with_db_path_and_metadata_dir(
                &graph_dir,
                &db_path,
                &metadata_dir,
            )
                .expect("Failed to initialize graph");

            let should_seed_tutorial = graph_dir == default_graph_dir;
            if should_seed_tutorial {
                match seed_tutorial_graph(&graph_dir, &metadata_dir) {
                    Ok(true) => {
                        if let Err(error) = graph.reindex_all() {
                            eprintln!("Warning: Welcome graph indexing failed: {error}");
                        }
                    }
                    Ok(false) => {}
                    Err(error) => eprintln!("Warning: Welcome graph seeding failed: {error}"),
                }
            }

            // Keep startup responsive. If DB is empty (first run or recovered),
            // rebuild in the background instead of blocking app initialization.
            // Use a cheap existence probe — a full page listing here would scan
            // the whole table and freeze the UI thread on very large graphs.
            if graph.needs_startup_reindex().unwrap_or(true) {
                let graph_dir_clone = graph_dir.clone();
                let db_path_clone = db_path.clone();
                let metadata_dir_clone = metadata_dir.clone();
                thread::spawn(move || {
                    match Graph::open_with_db_path_and_metadata_dir(
                        &graph_dir_clone,
                        &db_path_clone,
                        &metadata_dir_clone,
                    ) {
                        Ok(g) => {
                            if let Err(e) = g.reconcile_files_from_disk() {
                                eprintln!(
                                    "Warning: background startup file reconcile failed for '{}': {}",
                                    graph_dir_clone.display(),
                                    e
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: background startup reindex could not open '{}': {}",
                                graph_dir_clone.display(),
                                e
                            );
                        }
                    }
                });
            }

            // One-time (per graph): map existing FTS rows to their rowids.
            // `fts_blocks.block_id` is UNINDEXED, so deleting a block's FTS row
            // by block_id full-scans the whole index — seconds per edit on a
            // large graph, which froze the UI. `fts_block_rowid` lets edits
            // delete by rowid instead. Backfill runs in the background so it
            // never blocks startup; edits work meanwhile (only not-yet-mapped
            // legacy blocks use the slow path until the backfill reaches them).
            {
                let db_path_str = db_path.to_string_lossy().to_string();
                thread::spawn(move || match grafium_core::Database::new(&db_path_str) {
                    Ok(db) => match db.backfill_fts_rowid_map() {
                        Ok(0) => {}
                        Ok(n) => eprintln!("fts rowid map backfill: mapped {n} blocks"),
                        Err(e) => eprintln!("Warning: fts rowid map backfill failed: {}", e),
                    },
                    Err(e) => {
                        eprintln!("Warning: fts rowid map backfill could not open db: {}", e)
                    }
                });
            }

            // Register default graph in config if not present
            let mut config = config;
            let path_str = graph_dir.to_string_lossy().to_string();
            let name = if should_seed_tutorial {
                "Welcome Graph".to_string()
            } else {
                graph_dir.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("graph")
                    .to_string()
            };

            if let Some(existing) = config.graphs.iter_mut().find(|g| g.path == path_str) {
                existing.name = name.clone();
            } else {
                config.add_graph(&name, &path_str);
            }
            if config.current.is_none() {
                config.current = Some(path_str);
            }
            config
                .save(&config_path)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

            let state = AppState {
                graph: Arc::new(Mutex::new(graph)),
                watcher: Mutex::new(None),
            };
            state.restart_graph_watcher().expect("Failed to start graph watcher");

            // Start sync monitor (checks for USB/mount availability)
            let sync_graph = state.graph.clone();
            let sync_app_handle = app.handle().clone();
            start_sync_monitor(sync_app_handle, sync_graph);

            // Keep a handle for the auto-reindex drainer before the state is moved.
            let drainer_graph = state.graph.clone();

            // Start smplos theme watcher
            let theme_app_handle = app.handle().clone();
            start_smplos_theme_watcher(theme_app_handle);

            app.manage(state);
            app.manage(commands::jobs::JobsState::new());

            // Initialize Knowledge Engine
            let knowledge_state = {
                let data_dir = app_dir.join("knowledge");
                let config_path = data_dir.join("ai_config.json");
                let ai_config = if config_path.exists() {
                    std::fs::read_to_string(&config_path)
                        .ok()
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or_default()
                } else {
                    grafium_core::ai::config::AiConfig::default()
                };
                let engine = grafium_core::KnowledgeEngine::new_with_models_root(
                    &data_dir,
                    ai_config,
                    &app_dir,
                )
                    .ok();
                commands::knowledge::KnowledgeState {
                    engine: Arc::new(tokio::sync::RwLock::new(engine)),
                    cancels: Default::default(),
                }
            };
            // Keep the vector index fresh automatically as the graph changes.
            start_reindex_drainer(
                app.handle().clone(),
                drainer_graph,
                knowledge_state.engine.clone(),
            );
            app.manage(knowledge_state);

            // On Linux, intercept Ctrl+Z/Shift+Z at the GtkWindow level
            // WebKitGTK intercepts these keys internally before JS sees them,
            // and the WebView widget signal doesn't fire. By connecting to the
            // toplevel GtkWindow, we intercept BEFORE WebKitGTK processes them.
            #[cfg(target_os = "linux")]
            {
                if let Some(window) = app.get_webview_window("main") {
                    let win_for_eval = window.clone();
                    if let Err(err) = window.with_webview(move |webview| {
                        use gtk::prelude::*;

                        let wk_webview = webview.inner();
                        // Get the toplevel GtkWindow - key events go here first
                        if let Some(toplevel) = wk_webview.toplevel() {
                            if let Ok(gtk_window) = toplevel.downcast::<gtk::Window>() {
                                let eval_window = win_for_eval.clone();
                                gtk_window.connect_key_press_event(move |_, event| {
                                    let state = event.state();
                                    let keyval = event.keyval();
                                    let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
                                    let shift = state.contains(gdk::ModifierType::SHIFT_MASK);

                                    tracing::trace!("key_press: keyval={} ctrl={} shift={}", *keyval, ctrl, shift);

                                    // Ctrl+. toggles reference panel
                                    if ctrl && !shift && *keyval == 46 /* period */ {
                                        tracing::trace!("=> Ctrl+. detected, toggling reference panel");
                                        let _ = eval_window.eval("window.__toggleReferencePanel && window.__toggleReferencePanel()");
                                        return gtk::glib::Propagation::Stop;
                                    }

                                    if ctrl && !shift && (keyval == gdk::keys::constants::z || keyval == gdk::keys::constants::Z) {
                                        tracing::trace!("=> Ctrl+Z detected, calling eval(__handleNativeUndo)");
                                        // Call undo AND capture console output via a self-reporting mechanism
                                        let result = eval_window.eval(r#"
                                            (function() {
                                                var msg = 'EVAL_OK: __handleNativeUndo exists=' + (typeof window.__handleNativeUndo) + ' activeView=' + !!(window.__activeEditorView);
                                                var el = document.getElementById('__dbg');
                                                if (el) { el.innerHTML += '<br>' + msg; }
                                                if (window.__handleNativeUndo) {
                                                    window.__handleNativeUndo();
                                                } else {
                                                    document.title = 'ERROR: __handleNativeUndo not found!';
                                                }
                                            })();
                                        "#);
                                        tracing::trace!("=> eval result: {:?}", result);
                                        return gtk::glib::Propagation::Stop;
                                    }
                                    if ctrl && shift && (keyval == gdk::keys::constants::z || keyval == gdk::keys::constants::Z) {
                                        tracing::trace!("=> Ctrl+Shift+Z detected, calling eval(__handleNativeRedo)");
                                        let _ = eval_window.eval("window.__handleNativeRedo && window.__handleNativeRedo()");
                                        return gtk::glib::Propagation::Stop;
                                    }
                                    if ctrl && (keyval == gdk::keys::constants::y || keyval == gdk::keys::constants::Y) {
                                        tracing::trace!("=> Ctrl+Y detected, calling eval(__handleNativeRedo)");
                                        let _ = eval_window.eval("window.__handleNativeRedo && window.__handleNativeRedo()");
                                        return gtk::glib::Propagation::Stop;
                                    }
                                    if shift && !ctrl && (keyval == gdk::keys::constants::Up || keyval == gdk::keys::constants::Down) {
                                        let direction = if keyval == gdk::keys::constants::Up {
                                            "up"
                                        } else {
                                            "down"
                                        };
                                        tracing::trace!("=> Shift+Arrow{} detected, calling eval(__handleNativeVerticalArrow)", direction);
                                        let script = format!(
                                            "window.__handleNativeVerticalArrow && window.__handleNativeVerticalArrow('{}', true)",
                                            direction
                                        );
                                        let _ = eval_window.eval(&script);
                                        return gtk::glib::Propagation::Stop;
                                    }
                                    gtk::glib::Propagation::Proceed
                                });
                            } else {
                                eprintln!("Warning: Linux shortcut setup skipped because the webview toplevel is not a GtkWindow");
                            }
                        } else {
                            eprintln!("Warning: Linux shortcut setup skipped because the webview toplevel is unavailable");
                        }

                        // ALSO connect directly on the WebView widget itself
                        let eval_window2 = win_for_eval.clone();
                        wk_webview.connect_key_press_event(move |_, event| {
                            let state = event.state();
                            let keyval = event.keyval();
                            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
                            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);

                            tracing::trace!("key_press: keyval={} ctrl={} shift={}", *keyval, ctrl, shift);

                            if ctrl && !shift && (keyval == gdk::keys::constants::z || keyval == gdk::keys::constants::Z) {
                                tracing::trace!("=> Ctrl+Z detected, calling eval(__handleNativeUndo)");
                                let _ = eval_window2.eval("window.__handleNativeUndo && window.__handleNativeUndo()");
                                return gtk::glib::Propagation::Stop;
                            }
                            if ctrl && shift && (keyval == gdk::keys::constants::z || keyval == gdk::keys::constants::Z) {
                                tracing::trace!("=> Ctrl+Shift+Z detected");
                                let _ = eval_window2.eval("window.__handleNativeRedo && window.__handleNativeRedo()");
                                return gtk::glib::Propagation::Stop;
                            }
                            if ctrl && (keyval == gdk::keys::constants::y || keyval == gdk::keys::constants::Y) {
                                tracing::trace!("=> Ctrl+Y detected");
                                let _ = eval_window2.eval("window.__handleNativeRedo && window.__handleNativeRedo()");
                                return gtk::glib::Propagation::Stop;
                            }
                            if shift && !ctrl && (keyval == gdk::keys::constants::Up || keyval == gdk::keys::constants::Down) {
                                let direction = if keyval == gdk::keys::constants::Up {
                                    "up"
                                } else {
                                    "down"
                                };
                                tracing::trace!("=> Shift+Arrow{} detected", direction);
                                let script = format!(
                                    "window.__handleNativeVerticalArrow && window.__handleNativeVerticalArrow('{}', true)",
                                    direction
                                );
                                let _ = eval_window2.eval(&script);
                                return gtk::glib::Propagation::Stop;
                            }
                            gtk::glib::Propagation::Proceed
                        });
                    }) {
                        eprintln!(
                            "Warning: failed to access the main webview for Linux shortcut setup: {}",
                            err
                        );
                    }
                } else {
                    eprintln!(
                        "Warning: Linux shortcut setup skipped because the main webview window is unavailable"
                    );
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::startup::reveal_startup_window,
            commands::layout::get_sidebar_visibility,
            commands::layout::set_sidebar_visibility,
            commands::layout::get_layout_preferences,
            commands::layout::set_layout_preferences,
            commands::pages::list_pages,
            commands::pages::list_page_summaries,
            commands::pages::count_pages,
            commands::pages::list_pages_window,
            commands::pages::list_journal_pages,
            commands::pages::list_journal_note_dates,
            commands::pages::get_note_edit_counts,
            commands::pages::get_note_edits_for_day,
            commands::pages::get_page,
            commands::pages::create_page,
            commands::pages::update_page_meta,
            commands::pages::rename_page,
            commands::pages::bulk_rename_pages,
            commands::pages::delete_page,
            commands::pages::delete_namespace,
            commands::pages::delete_book_folder,
            commands::pages::open_page_in_file_browser,
            commands::pages::open_namespace_in_file_browser,
            commands::pages::open_book_folder_in_file_browser,
            commands::pages::get_page_source,
            commands::pages::update_page_source,
            commands::pages::get_parent_page,
            commands::pages::get_child_pages,
            commands::pages::search_page_titles,
            commands::trees::pages_namespace_tree,
            commands::trees::pages_tag_tree,
            commands::trees::page_set_collection,
            commands::trees::pages_list_collections,
            commands::blocks::list_blocks,
            commands::blocks::get_block,
            commands::blocks::create_block,
            commands::blocks::create_blocks,
            commands::blocks::update_block,
            commands::blocks::delete_block,
            commands::blocks::delete_blocks,
            commands::blocks::move_block,
            commands::blocks::reorder_blocks,
            commands::blocks::get_block_page_title,
            commands::blocks::search_fts,
            commands::links::get_backlinks,
            commands::links::discover_link_candidates,
            commands::links::list_link_candidates,
            commands::links::accept_link_candidate,
            commands::links::resolve_link_candidate,
            commands::links::dismiss_link_candidate,
            commands::links::restore_link_candidate,
            commands::links::undo_link_candidate_accept,
            commands::tasks::list_tasks,
            commands::tasks::update_task_state,
            commands::tasks::cycle_task_state,
            commands::tasks::get_completion_counts,
            commands::tasks::get_completed_tasks,
            commands::tasks::get_open_tasks,
            commands::tasks::set_task_date,
            commands::tasks::list_open_task_rows,
            commands::tasks::task_flow_stats,
            commands::tasks::backfill_task_completions,
            commands::assistant::handle_assistant_command,
            commands::flashcards::list_flashcards_due,
            commands::flashcards::list_flashcard_topics,
            commands::flashcards::list_all_flashcards,
            commands::flashcards::update_flashcard_review,
            commands::flashcards::grade_flashcard,
            commands::flashcards::import_anki_apkg,
            commands::books::books_import_directory,
            commands::favorites::add_favorite,
            commands::favorites::remove_favorite,
            commands::favorites::list_favorites,
            commands::favorites::record_page_open,
            commands::favorites::list_recent_pages,
            commands::chat::list_chat_threads,
            commands::chat::load_chat_thread,
            commands::chat::save_chat_thread,
            commands::chat::rename_chat_thread,
            commands::chat::delete_chat_thread,
            commands::chat::chat_concurrency,
            commands::query::run_query,
            commands::query::get_property_keys,
            commands::query::get_property_values,
            commands::graph::get_graph_info,
            commands::graph::get_graph_data,
            commands::graph::list_graphs,
            commands::graph::open_graph,
            commands::graph::create_graph,
            commands::graph::validate_graph,
            commands::graph::reindex_current,
            commands::graph::remove_graph,
            commands::graph::get_app_version,
            commands::graph::list_directory,
            commands::graph::get_default_graph_base,
            commands::graph::get_tutorial_graph_path,
            commands::help::help_get_page,
            commands::sync::sync_list_targets,
            commands::sync::sync_add_filesystem_target,
            commands::sync::sync_add_webdav_target,
            commands::sync::sync_remove_target,
            commands::sync::sync_check_status,
            commands::sync::sync_list_conflicts,
            commands::sync::sync_run,
            commands::sync::sync_run_all,
            commands::theme::get_smplos_theme,
            commands::theme::get_smplos_theme_colors,
            commands::theme::get_app_theme,
            commands::theme::set_app_theme,
            commands::assets::download_asset,
            commands::assets::list_assets,
            commands::assets::read_asset_data_url,
            commands::assets::resolve_asset_file_path,
            commands::assets::save_image_to_path,
            commands::assets::find_orphaned_assets,
            commands::assets::delete_assets,
            commands::knowledge::ai_get_config,
            commands::knowledge::ai_default_concept_edge_prompt,
            commands::knowledge::ai_set_config,
            commands::knowledge::ai_health_check,
            commands::knowledge::ai_index_page,
            commands::knowledge::ai_index_all_pages,
            commands::knowledge::ai_index_status,
            commands::knowledge::ai_retry_llm_on_gpu,
            commands::knowledge::ai_search,
            commands::knowledge::ai_generate_references,
            commands::knowledge::ai_cancel_operation,
            commands::knowledge::ai_summarize_selection,
            commands::writing::ai_analyze_writing,
            commands::writing::ai_rewrite_writing,
            commands::writing::ai_cancel_writing,
            commands::writing_edits::apply_writing_changes,
            commands::reading_notes::reading_notes_list,
            commands::reading_notes::reading_note_create,
            commands::reading_notes::reading_note_update,
            commands::reading_notes::reading_note_reattach,
            commands::knowledge::ai_create_concept_edges,
            commands::knowledge::ai_research_web,
            commands::research::research_get_config,
            commands::research::research_set_config,
            commands::research::research_reset_prompts,
            commands::research::research_test_engine,
            commands::research::research_deep,
            commands::research::research_scope_info,
            commands::assistant::assistant_context_info,
            commands::assistant::assistant_chat,
            commands::research::research_scoped,
            commands::research::research_cancel,
            commands::knowledge::text_wrap_known_terms,
            commands::knowledge::ai_insert_page_summary,
            commands::knowledge::ai_undo_summary_insert,
            commands::knowledge::ai_reapply_summary_insert,
            commands::knowledge::ai_ask,
            commands::knowledge::ai_ask_stream,
            commands::knowledge::get_chat_preferences,
            commands::knowledge::set_chat_preferences,
            commands::knowledge::ai_cancel_stream,
            commands::knowledge::ai_list_registered_graphs,
            commands::knowledge::ai_register_graph,
            commands::knowledge::ai_list_schemas,
            commands::knowledge::ai_save_schema,
            debug_log,
            commands::knowledge::ai_create_default_schemas,
            commands::media::media_import_video,
            commands::jobs::jobs_list,
            commands::jobs::jobs_cancel,
            commands::jobs::jobs_clear_finished,
            commands::media::media_get_config,
            commands::media::media_set_config,
            commands::ui_log,
            commands::model_library::list_local_models,
            commands::model_library::detect_gpu_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
