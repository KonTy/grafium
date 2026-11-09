use crate::commands::theme_native_tests::javascript;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Manager, WebviewWindow};

async fn exercise(
    window: WebviewWindow,
    graph: Arc<Mutex<grafium_core::Graph>>,
    page_id: String,
) -> Result<(), String> {
    for _ in 0..100 {
        if javascript(&window, "!!document.querySelector('.sidebar')").await? == "true" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    javascript(
        &window,
        "window.dispatchEvent(new CustomEvent('navigate-page', {detail:'Paste latency'})); 'ok'",
    )
    .await?;
    for _ in 0..100 {
        if javascript(&window, "!!document.querySelector('.block-content')").await? == "true" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    javascript(
        &window,
        "document.querySelector('.block-content').click(); 'ok'",
    )
    .await?;
    if javascript(&window, "!!window.__activeEditorView").await? != "true" {
        return Err("The real block editor did not open".into());
    }
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let held = graph.clone();
    let blocker = std::thread::spawn(move || {
        let _guard = held.lock().unwrap();
        locked_tx.send(()).unwrap();
        let _ = release_rx.recv_timeout(Duration::from_secs(3));
    });
    locked_rx.await.map_err(|e| e.to_string())?;
    let responsive = tokio::time::timeout(Duration::from_millis(400), async {
        javascript(&window, r#"(() => {
        const data = new DataTransfer();
        data.setData('text/plain', 'Work through the entire agreed backlog autonomously.\n\nFirst enumerate every item so none disappear during the work.');
        document.querySelector('.cm-content').dispatchEvent(new ClipboardEvent('paste', {
            clipboardData: data, bubbles: true, cancelable: true
        }));
        return 'ok';
    })()"#).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        javascript(&window, "document.querySelector('.cm-content').isContentEditable").await
    }).await;
    // Release even when the main-thread probe failed, so teardown cannot deadlock.
    let _ = release_tx.send(());
    blocker.join().map_err(|_| "Graph blocker panicked")?;
    if responsive.map_err(|_| "Paste blocked the native UI while waiting for the graph lock")??
        != "true"
    {
        return Err("The editor stopped being writable during paste".into());
    }
    for _ in 0..100 {
        let saved = {
            let graph = graph.lock().map_err(|e| e.to_string())?;
            graph
                .db
                .list_blocks_for_page(&page_id)
                .map_err(|e| e.to_string())?
        };
        if saved.len() == 2 && saved[1].content.starts_with("First enumerate") {
            let graph = graph.lock().map_err(|e| e.to_string())?;
            let source = std::fs::read_to_string(graph.pages_dir.join("Paste latency.md"))
                .map_err(|e| e.to_string())?;
            if !source.contains("Work through") || !source.contains("First enumerate") {
                return Err("The native paste did not persist both paragraphs to Markdown".into());
            }
            println!("PASS native Svelte/WebKit paste remains editable during graph contention and persists both blocks");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err("Pasted paragraphs were not saved through native IPC".into())
}

#[test]
#[ignore = "requires WebKitGTK and a display; run alone with xvfb-run"]
fn native_paste_does_not_block_ui_on_graph_lock() {
    let home = tempfile::tempdir().unwrap();
    for (name, path) in [
        ("HOME", home.path().join("home")),
        ("XDG_CONFIG_HOME", home.path().join("config")),
        ("XDG_DATA_HOME", home.path().join("data")),
        ("XDG_CACHE_HOME", home.path().join("cache")),
    ] {
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(name, path);
    }
    let graph = grafium_core::Graph::open(&home.path().join("graph")).unwrap();
    let page = graph.create_page("Paste latency", false).unwrap();
    let graph = Arc::new(Mutex::new(graph));
    crate::webkit_renderer::configure();
    gtk::glib::set_prgname(Some("grafium"));
    let result = Arc::new(Mutex::new(None));
    let outcome = result.clone();
    let test_graph = graph.clone();
    let app = tauri::Builder::default()
        .any_thread()
        .manage(crate::commands::startup::StartupWindow::default())
        .manage(crate::AppState {
            graph,
            watcher: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            crate::commands::theme::get_app_theme,
            crate::commands::theme::get_system_appearance,
            crate::commands::startup::reveal_startup_window,
            crate::commands::layout::get_layout_preferences,
            crate::commands::graph::get_graph_info,
            crate::commands::pages::get_page,
            crate::commands::pages::create_page,
            crate::commands::pages::list_pages,
            crate::commands::pages::list_page_summaries,
            crate::commands::pages::count_pages,
            crate::commands::pages::list_journal_pages,
            crate::commands::pages::get_parent_page,
            crate::commands::pages::get_child_pages,
            crate::commands::blocks::list_blocks,
            crate::commands::blocks::update_block,
            crate::commands::blocks::create_blocks,
            crate::commands::blocks::reorder_blocks,
            crate::commands::links::get_backlinks,
            crate::commands::favorites::list_favorites,
            crate::commands::favorites::record_page_open,
        ])
        .setup(move |app| {
            let window = app.get_webview_window("main").unwrap();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let tested = exercise(window, test_graph, page.id).await;
                *outcome.lock().unwrap() = Some(tested);
                handle.exit(0);
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .unwrap();
    assert_eq!(app.run_return(|_, _| {}), 0);
    result
        .lock()
        .unwrap()
        .take()
        .expect("Native paste test did not finish")
        .unwrap();
}
