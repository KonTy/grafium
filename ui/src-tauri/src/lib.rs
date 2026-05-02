mod commands;

use commands::graph::GraphConfig;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use pkm_core::Graph;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use tauri::Manager;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_dir = app.path().app_data_dir().expect("Failed to get app data dir");
            let config_path = app_dir.join("graphs.json");
            let config = GraphConfig::load(&config_path);

            // Use last-used graph, or fall back to default graph dir
            let graph_dir = if let Some(ref current) = config.current {
                PathBuf::from(current)
            } else {
                app_dir.join("graph")
            };

            let graph = Graph::open(&graph_dir)
                .expect("Failed to initialize graph");

            // Only reindex if DB is empty (first run or corrupted)
            let page_count = graph.db.list_pages(100, 0).map(|p| p.len()).unwrap_or(0);
            if page_count == 0 {
                if let Err(e) = graph.reindex_all() {
                    eprintln!("Warning: reindex failed: {}", e);
                }
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
            app.manage(state);

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

                    // Log ALL key events at GTK level to see what arrives
                    let eval_window = win_for_eval.clone();
                    gtk_window.connect_key_press_event(move |_, event| {
                        let state = event.state();
                        let keyval = event.keyval();
                        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
                        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);

                        eprintln!("[GTK-WINDOW] key_press: keyval={} ctrl={} shift={}", *keyval, ctrl, shift);

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
            commands::blocks::search_fts,
            commands::links::get_backlinks,
            commands::tasks::list_tasks,
            commands::tasks::update_task_state,
            commands::tasks::cycle_task_state,
            commands::flashcards::list_flashcards_due,
            commands::flashcards::list_all_flashcards,
            commands::flashcards::update_flashcard_review,
            commands::favorites::add_favorite,
            commands::favorites::remove_favorite,
            commands::favorites::list_favorites,
            commands::favorites::record_page_open,
            commands::favorites::list_recent_pages,
            commands::query::run_query,
            commands::graph::get_graph_info,
            commands::graph::list_graphs,
            commands::graph::open_graph,
            commands::graph::create_graph,
            commands::graph::reindex_current,
            commands::graph::remove_graph,
            commands::graph::get_app_version,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
