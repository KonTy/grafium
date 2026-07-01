mod commands;

use commands::graph::GraphConfig;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use grafium_core::Graph;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

pub struct AppState {
    pub graph: Arc<Mutex<Graph>>,
    watcher: Mutex<Option<GraphWatcherHandle>>,
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

        let (pages_dir, journals_dir) = {
            let graph = self.graph.lock().map_err(|e| e.to_string())?;
            (graph.pages_dir.clone(), graph.journals_dir.clone())
        };

        let handle = start_graph_watcher(self.graph.clone(), pages_dir, journals_dir)?;
        let mut guard = self.watcher.lock().map_err(|e| e.to_string())?;
        *guard = Some(handle);
        Ok(())
    }
}

fn should_process_event(event: &Event, pages_dir: &std::path::Path, journals_dir: &std::path::Path) -> bool {
    event.paths.iter().any(|p| {
        p.extension().and_then(|e| e.to_str()) == Some("md")
            && (p.starts_with(pages_dir) || p.starts_with(journals_dir))
    })
}

fn is_full_reindex_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Remove(_) | EventKind::Create(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
    )
}

fn start_graph_watcher(
    graph: Arc<Mutex<Graph>>,
    pages_dir: PathBuf,
    journals_dir: PathBuf,
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

        let debounce = Duration::from_millis(400);
        let mut pending_files = std::collections::HashSet::<PathBuf>::new();
        let mut pending_full_reindex = false;
        let mut last_event_at: Option<Instant> = None;

        loop {
            if stop_rx.try_recv().is_ok() {
                break;
            }

            match event_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(event)) => {
                    if !should_process_event(&event, &pages_dir, &journals_dir) {
                        continue;
                    }

                    if is_full_reindex_event(&event.kind) {
                        pending_full_reindex = true;
                    } else {
                        for path in event.paths {
                            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                                pending_files.insert(path);
                            }
                        }
                    }
                    last_event_at = Some(Instant::now());
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
                    pending_full_reindex = false;
                    last_event_at = None;
                    continue;
                }
            };

            if pending_full_reindex {
                if let Err(e) = g.reindex_all() {
                    eprintln!("watch reindex failed: {}", e);
                }
            } else {
                let mut missing_file_seen = false;
                for path in pending_files.drain() {
                    if path.exists() {
                        if let Err(e) = g.index_file(&path) {
                            eprintln!("watch index file failed ({}): {}", path.display(), e);
                        }
                    } else {
                        missing_file_seen = true;
                    }
                }

                // Some platforms report deletes as generic modify events.
                // If a watched markdown path disappeared, run a full reconcile.
                if missing_file_seen {
                    if let Err(e) = g.reindex_all() {
                        eprintln!("watch fallback reindex failed: {}", e);
                    }
                }
            }

            pending_files.clear();
            pending_full_reindex = false;
            last_event_at = None;
        }
    });

    Ok(GraphWatcherHandle { stop_tx, join_handle })
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

        let mut last_content = fs::read_to_string(&theme_path).unwrap_or_default().trim().to_string();

        loop {
            thread::sleep(Duration::from_secs(2));

            let current = match fs::read_to_string(&theme_path) {
                Ok(s) => s.trim().to_string(),
                Err(_) => continue,
            };

            if current != last_content && !current.is_empty() {
                eprintln!("[theme-watcher] smplos theme changed: {} -> {}", last_content, current);
                last_content = current.clone();
                let _ = app_handle.emit("smplos-theme-changed", serde_json::json!({
                    "theme": current,
                }));
            }
        }
    });
}

/// Background thread that periodically checks if sync targets become available.
/// When a target that was unavailable becomes available, it emits a Tauri event
/// and optionally triggers auto-sync.
fn start_sync_monitor(
    app_handle: tauri::AppHandle,
    graph: Arc<Mutex<Graph>>,
) {
    use grafium_core::sync::{
        SyncEngine,
        filesystem::FilesystemBackend,
        webdav::WebDavBackend,
        state::{SyncConfigs, BackendConfig},
    };

    thread::spawn(move || {
        let mut was_available: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
        let check_interval = Duration::from_secs(5);

        loop {
            thread::sleep(check_interval);

            let root_dir = match graph.lock() {
                Ok(g) => g.root_dir.clone(),
                Err(_) => continue,
            };

            let config_path = root_dir.join(".logseq").join("sync-config.json");
            let configs = SyncConfigs::load(&config_path);

            for target in &configs.targets {
                let backend: Box<dyn grafium_core::sync::SyncBackend> = match &target.config {
                    BackendConfig::Filesystem { path } => {
                        Box::new(FilesystemBackend::new(path.clone(), target.name.clone()))
                    }
                    BackendConfig::WebDav { url, username, password } => {
                        Box::new(WebDavBackend::new(
                            url.clone(), username.clone(), password.clone(), target.name.clone(),
                        ))
                    }
                };

                let now_available = backend.is_available();
                let previously_available = was_available.get(&target.id).copied().unwrap_or(false);

                if now_available && !previously_available {
                    // Target just became available!
                    eprintln!("[sync-monitor] Target '{}' is now available", target.name);

                    // Emit event to frontend
                    let _ = app_handle.emit("sync-target-available", serde_json::json!({
                        "target_id": target.id,
                        "target_name": target.name,
                    }));

                    // Auto-sync if enabled
                    if target.auto_sync {
                        let engine = SyncEngine::new(root_dir.clone());
                        match engine.sync(backend.as_ref()) {
                            Ok(result) => {
                                eprintln!("[sync-monitor] Auto-sync '{}': {}", target.name, result.summary());

                                // Reindex if we pulled files
                                if !result.pulled.is_empty() || !result.conflicts.is_empty() || !result.deleted_local.is_empty() {
                                    if let Ok(g) = graph.lock() {
                                        let _ = g.reindex_all();
                                    }
                                    // Notify frontend to refresh
                                    let _ = app_handle.emit("sync-completed", serde_json::json!({
                                        "target_name": target.name,
                                        "pushed": result.pushed.len(),
                                        "pulled": result.pulled.len(),
                                        "conflicts": result.conflicts.len(),
                                    }));
                                }
                            }
                            Err(e) => {
                                eprintln!("[sync-monitor] Auto-sync '{}' failed: {}", target.name, e);
                                let _ = app_handle.emit("sync-error", serde_json::json!({
                                    "target_name": target.name,
                                    "error": e.to_string(),
                                }));
                            }
                        }
                    }
                } else if !now_available && previously_available {
                    // Target disconnected
                    let _ = app_handle.emit("sync-target-disconnected", serde_json::json!({
                        "target_id": target.id,
                        "target_name": target.name,
                    }));
                }

                was_available.insert(target.id.clone(), now_available);
            }
        }
    });
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
        let _ = app;
        graph_root.join(".logseq").join("index.db")
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_dir = app.path().app_data_dir().expect("Failed to get app data dir");
            let config_path = app_dir.join("graphs.json");
            let config = GraphConfig::load(&config_path);

            // Use last-used graph, or fall back to default graph dir.
            // Validate the saved path before trusting it — if it no longer has a proper
            // graph structure, fall back to the default directory rather than creating
            // subdirectories inside an arbitrary user folder.
            let default_graph_dir = app_dir.join("graph");
            let graph_dir = if let Some(ref current) = config.current {
                let candidate = PathBuf::from(current);
                let validation = Graph::validate_structure(&candidate);
                if validation.is_valid {
                    candidate
                } else {
                    eprintln!(
                        "Warning: saved graph path '{}' is no longer valid ({}), falling back to default",
                        current,
                        validation.error_message.as_deref().unwrap_or("unknown error")
                    );
                    default_graph_dir
                }
            } else {
                default_graph_dir
            };

            let db_path = platform_db_path(app.handle(), &graph_dir);
            let graph = Graph::open_with_db_path(&graph_dir, &db_path)
                .expect("Failed to initialize graph");

            // Keep startup responsive. If DB is empty (first run or recovered),
            // rebuild in the background instead of blocking app initialization.
            let page_count = graph.db.list_pages(100, 0).map(|p| p.len()).unwrap_or(0);
            if page_count == 0 {
                let graph_dir_clone = graph_dir.clone();
                let db_path_clone = db_path.clone();
                thread::spawn(move || {
                    match Graph::open_with_db_path(&graph_dir_clone, &db_path_clone) {
                        Ok(g) => {
                            if let Err(e) = g.reindex_all() {
                                eprintln!(
                                    "Warning: background startup reindex failed for '{}': {}",
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

            // Register default graph in config if not present
            let mut config = config;
            let path_str = graph_dir.to_string_lossy().to_string();
            let name = graph_dir.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("graph")
                .to_string();
            config.add_graph(&name, &path_str);
            if config.current.is_none() {
                config.current = Some(path_str);
            }
            config.save(&config_path);

            let state = AppState {
                graph: Arc::new(Mutex::new(graph)),
                watcher: Mutex::new(None),
            };
            state.restart_graph_watcher().expect("Failed to start graph watcher");

            // Start sync monitor (checks for USB/mount availability)
            let sync_graph = state.graph.clone();
            let sync_app_handle = app.handle().clone();
            start_sync_monitor(sync_app_handle, sync_graph);

            // Start smplos theme watcher
            let theme_app_handle = app.handle().clone();
            start_smplos_theme_watcher(theme_app_handle);

            app.manage(state);

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
                let engine = grafium_core::KnowledgeEngine::new(&data_dir, ai_config).ok();
                commands::knowledge::KnowledgeState {
                    engine: Arc::new(tokio::sync::RwLock::new(engine)),
                }
            };
            app.manage(knowledge_state);

            // On Linux, intercept Ctrl+Z/Shift+Z at the GtkWindow level
            // WebKitGTK intercepts these keys internally before JS sees them,
            // and the WebView widget signal doesn't fire. By connecting to the
            // toplevel GtkWindow, we intercept BEFORE WebKitGTK processes them.
            #[cfg(target_os = "linux")]
            {
                let window = app.get_webview_window("main").unwrap();
                let win_for_eval = window.clone();
                window.with_webview(move |webview| {
                    use gtk::prelude::*;

                    let wk_webview = webview.inner();
                    // Get the toplevel GtkWindow - key events go here first
                    let toplevel = wk_webview.toplevel().unwrap();
                    let gtk_window = toplevel.downcast::<gtk::Window>().unwrap();

                    let eval_window = win_for_eval.clone();
                    gtk_window.connect_key_press_event(move |_, event| {
                        let state = event.state();
                        let keyval = event.keyval();
                        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
                        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);

                        eprintln!("[GTK-WINDOW] key_press: keyval={} ctrl={} shift={}", *keyval, ctrl, shift);

                        // Ctrl+. toggles reference panel
                        if ctrl && !shift && *keyval == 46 /* period */ {
                            eprintln!("[GTK-WINDOW] => Ctrl+. detected, toggling reference panel");
                            let _ = eval_window.eval("window.__toggleReferencePanel && window.__toggleReferencePanel()");
                            return gtk::glib::Propagation::Stop;
                        }

                        if ctrl && !shift && (keyval == gdk::keys::constants::z || keyval == gdk::keys::constants::Z) {
                            eprintln!("[GTK-WINDOW] => Ctrl+Z detected, calling eval(__handleNativeUndo)");
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
                            eprintln!("[GTK-WINDOW] => eval result: {:?}", result);
                            return gtk::glib::Propagation::Stop;
                        }
                        if ctrl && shift && (keyval == gdk::keys::constants::z || keyval == gdk::keys::constants::Z) {
                            eprintln!("[GTK-WINDOW] => Ctrl+Shift+Z detected, calling eval(__handleNativeRedo)");
                            let _ = eval_window.eval("window.__handleNativeRedo && window.__handleNativeRedo()");
                            return gtk::glib::Propagation::Stop;
                        }
                        if ctrl && (keyval == gdk::keys::constants::y || keyval == gdk::keys::constants::Y) {
                            eprintln!("[GTK-WINDOW] => Ctrl+Y detected, calling eval(__handleNativeRedo)");
                            let _ = eval_window.eval("window.__handleNativeRedo && window.__handleNativeRedo()");
                            return gtk::glib::Propagation::Stop;
                        }
                        gtk::glib::Propagation::Proceed
                    });

                    // ALSO connect directly on the WebView widget itself
                    let eval_window2 = win_for_eval.clone();
                    wk_webview.connect_key_press_event(move |_, event| {
                        let state = event.state();
                        let keyval = event.keyval();
                        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
                        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);

                        eprintln!("[GTK-WEBVIEW] key_press: keyval={} ctrl={} shift={}", *keyval, ctrl, shift);

                        if ctrl && !shift && (keyval == gdk::keys::constants::z || keyval == gdk::keys::constants::Z) {
                            eprintln!("[GTK-WEBVIEW] => Ctrl+Z detected, calling eval(__handleNativeUndo)");
                            let _ = eval_window2.eval("window.__handleNativeUndo && window.__handleNativeUndo()");
                            return gtk::glib::Propagation::Stop;
                        }
                        if ctrl && shift && (keyval == gdk::keys::constants::z || keyval == gdk::keys::constants::Z) {
                            eprintln!("[GTK-WEBVIEW] => Ctrl+Shift+Z detected");
                            let _ = eval_window2.eval("window.__handleNativeRedo && window.__handleNativeRedo()");
                            return gtk::glib::Propagation::Stop;
                        }
                        if ctrl && (keyval == gdk::keys::constants::y || keyval == gdk::keys::constants::Y) {
                            eprintln!("[GTK-WEBVIEW] => Ctrl+Y detected");
                            let _ = eval_window2.eval("window.__handleNativeRedo && window.__handleNativeRedo()");
                            return gtk::glib::Propagation::Stop;
                        }
                        gtk::glib::Propagation::Proceed
                    });
                }).expect("Failed to access webview");
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::pages::list_pages,
            commands::pages::list_journal_pages,
            commands::pages::get_page,
            commands::pages::create_page,
            commands::pages::update_page_meta,
            commands::pages::delete_page,
            commands::pages::get_parent_page,
            commands::pages::get_child_pages,
            commands::blocks::list_blocks,
            commands::blocks::create_block,
            commands::blocks::update_block,
            commands::blocks::delete_block,
            commands::blocks::move_block,
            commands::blocks::reorder_blocks,
            commands::blocks::get_block_page_title,
            commands::blocks::search_fts,
            commands::links::get_backlinks,
            commands::tasks::list_tasks,
            commands::tasks::update_task_state,
            commands::tasks::cycle_task_state,
            commands::tasks::get_completion_counts,
            commands::tasks::get_completed_tasks,
            commands::tasks::set_task_date,
            commands::flashcards::list_flashcards_due,
            commands::flashcards::list_all_flashcards,
            commands::flashcards::update_flashcard_review,
            commands::favorites::add_favorite,
            commands::favorites::remove_favorite,
            commands::favorites::list_favorites,
            commands::favorites::record_page_open,
            commands::favorites::list_recent_pages,
            commands::query::run_query,
            commands::query::get_property_keys,
            commands::query::get_property_values,
            commands::graph::get_graph_info,
            commands::graph::list_graphs,
            commands::graph::open_graph,
            commands::graph::create_graph,
            commands::graph::validate_graph,
            commands::graph::reindex_current,
            commands::graph::remove_graph,
            commands::graph::get_app_version,
            commands::graph::list_directory,
            commands::graph::get_default_graph_base,
            commands::sync::sync_list_targets,
            commands::sync::sync_add_filesystem_target,
            commands::sync::sync_add_webdav_target,
            commands::sync::sync_remove_target,
            commands::sync::sync_check_status,
            commands::sync::sync_run,
            commands::sync::sync_run_all,
            commands::theme::get_smplos_theme,
            commands::theme::get_smplos_theme_colors,
            commands::theme::get_app_theme,
            commands::theme::set_app_theme,
            commands::assets::download_asset,
            commands::assets::list_assets,
            commands::assets::find_orphaned_assets,
            commands::assets::delete_assets,
            commands::knowledge::ai_get_config,
            commands::knowledge::ai_set_config,
            commands::knowledge::ai_health_check,
            commands::knowledge::ai_index_page,
            commands::knowledge::ai_index_all_pages,
            commands::knowledge::ai_search,
            commands::knowledge::ai_generate_references,
            commands::knowledge::ai_ask,
            commands::knowledge::ai_list_registered_graphs,
            commands::knowledge::ai_register_graph,
            commands::knowledge::ai_list_schemas,
            commands::knowledge::ai_save_schema,
            commands::knowledge::ai_create_default_schemas,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
