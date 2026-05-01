mod commands;

use commands::graph::GraphConfig;
use pkm_core::Graph;
use std::sync::Mutex;
use std::path::PathBuf;
use tauri::Manager;

pub struct AppState {
    pub graph: Mutex<Graph>,
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

            app.manage(AppState { graph: Mutex::new(graph) });

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
