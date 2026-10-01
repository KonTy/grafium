use super::theme::get_system_appearance;
use gtk::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Emitter, Manager, WebviewWindow};
use webkit2gtk::WebViewExt;

#[allow(deprecated)]
async fn javascript(window: &WebviewWindow, script: &str) -> Result<String, String> {
    let script = script.to_owned();
    let (send, receive) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |webview| {
            webview.inner().run_javascript(
                &script,
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    let result = result.map_err(|error| error.to_string()).and_then(|value| {
                        value
                            .js_value()
                            .map(|value| value.to_string())
                            .ok_or("No JavaScript value".into())
                    });
                    let _ = send.send(result);
                },
            );
        })
        .map_err(|error| error.to_string())?;
    receive.await.map_err(|error| error.to_string())?
}

async fn painted_pixels(window: &WebviewWindow, alpha: u8) -> Result<(usize, usize), String> {
    let mut backgrounds = 0;
    let mut solid_text = 0;
    for pixel in native_frame(window).await?.chunks_exact(4) {
        let argb = u32::from_ne_bytes(pixel.try_into().unwrap());
        if ((argb >> 24) as u8).abs_diff(alpha) <= 1 {
            backgrounds += 1;
        }
        if argb == 0xff4c4f69 {
            solid_text += 1;
        }
    }
    Ok((backgrounds, solid_text))
}

async fn native_frame(window: &WebviewWindow) -> Result<Vec<u8>, String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |webview| {
            let native = webview
                .inner()
                .toplevel()
                .unwrap()
                .downcast::<gtk::Window>()
                .unwrap();
            if !native.is_drawable() {
                let _ = send.send(Ok(Vec::new()));
                return;
            }
            let clock = native.frame_clock().unwrap();
            let handler = std::rc::Rc::new(std::cell::RefCell::new(None));
            let handler_on_frame = handler.clone();
            let send = std::cell::RefCell::new(Some(send));
            let native_on_frame = native.clone();
            // A queued resize/remap has not completed when with_webview runs.
            // Measure after GTK's layout/paint phase, never during allocation.
            *handler.borrow_mut() = Some(clock.connect_after_paint(move |clock| {
                clock.disconnect(handler_on_frame.borrow_mut().take().unwrap());
                let result = capture_frame(&native_on_frame);
                let _ = send.borrow_mut().take().unwrap().send(result);
            }));
            native.queue_draw();
            clock.request_phase(gdk::FrameClockPhase::AFTER_PAINT);
        })
        .map_err(|error| error.to_string())?;
    tokio::time::timeout(Duration::from_secs(5), receive)
        .await
        .map_err(|_| "GTK frame did not finish")?
        .map_err(|error| error.to_string())?
}

fn capture_frame(native: &gtk::Window) -> Result<Vec<u8>, String> {
    let mut surface = gtk::cairo::ImageSurface::create(
        gtk::cairo::Format::ARgb32,
        native.allocated_width(),
        native.allocated_height(),
    )
    .map_err(|error| error.to_string())?;
    {
        let context = gtk::cairo::Context::new(&surface).map_err(|error| error.to_string())?;
        // WebKit get_snapshot bypasses its on-screen backing store and
        // passed even when every pixel of this GTK draw was opaque.
        native.draw(&context);
    }
    let pixels = surface.data().map_err(|error| error.to_string())?.to_vec();
    Ok(pixels)
}

async fn scrolling(window: &WebviewWindow) -> Result<(), String> {
    javascript(window, "window.dispatchEvent(new CustomEvent('navigate-page', {detail:'Scrolling fixture'})); 'ok'").await?;
    for _ in 0..100 {
        if javascript(
            window,
            "document.querySelectorAll('.rendered-content').length > 20",
        )
        .await?
            == "true"
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let found = javascript(window, r#"(() => {
        let el = document.querySelector('.rendered-content');
        while (el && !(el.scrollHeight > el.clientHeight + 100 && /auto|scroll/.test(getComputedStyle(el).overflowY))) el = el.parentElement;
        window.__appearanceScroller = el;
        return !!el;
    })()"#).await?;
    if found != "true" {
        return Err("No real page scroll container".into());
    }
    for editing in [false, true] {
        if editing {
            javascript(
                window,
                "document.querySelectorAll('.rendered-content')[1].click(); 'ok'",
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
            if javascript(window, "!!document.querySelector('.cm-content')").await? != "true" {
                return Err("Real block editor did not open".into());
            }
        }
        javascript(window, "window.__appearanceScroller.scrollTop = 0; 'ok'").await?;
        tokio::time::sleep(Duration::from_millis(350)).await;
        let baseline = native_frame(window).await?;
        for offset in [300, 1000, 600, 1400, 200, 0, 600, 0] {
            javascript(
                window,
                &format!("window.__appearanceScroller.scrollTop = {offset}; 'ok'"),
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(150)).await;
            if offset != 0
                && javascript(window, "window.__appearanceScroller.scrollTop > 0").await? != "true"
            {
                return Err("The document did not scroll".into());
            }
        }
        tokio::time::sleep(Duration::from_millis(350)).await;
        let after = native_frame(window).await?;
        if after.len() != baseline.len() || baseline.is_empty() {
            return Err("Native frame geometry changed during scroll comparison".into());
        }
        let changed = baseline
            .chunks_exact(4)
            .zip(after.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        // Allow a blinking editor caret / scrollbar fade, not old text trails.
        if changed > baseline.len() / 4 / 1000 {
            return Err(format!("Native scroll damage: editing={editing}, {changed} changed pixels at the original position"));
        }
        println!("PASS real reader/editor scrolling editing={editing}: {changed} changed pixels after eight scrolls");
    }
    Ok(())
}

async fn exercise(window: WebviewWindow, colors: std::path::PathBuf) -> Result<(), String> {
    let legacy = std::env::var_os("GRAFIUM_TEST_LEGACY_RENDERER").is_some();
    let appearance = get_system_appearance(window.clone()).await;
    println!(
        "production appearance: {}",
        serde_json::to_string(&appearance).unwrap()
    );
    if appearance.native_transparency == legacy {
        return Err("Native fixture capability did not match its renderer".into());
    }
    for _ in 0..100 {
        if javascript(&window, "!!document.querySelector('.sidebar')").await? == "true"
            && window.is_visible().map_err(|error| error.to_string())?
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    javascript(
        &window,
        "window.dispatchEvent(new CustomEvent('navigate-page', {detail:'__settings__'})); 'ok'",
    )
    .await?;
    for (index, opacity) in [0.9, 0.0, 0.5, 1.0, 0.5, 0.9].into_iter().enumerate() {
        std::fs::write(&colors, format!("app_background_opacity = {opacity}\n"))
            .map_err(|error| error.to_string())?;
        window
            .emit("smplos-theme-changed", ())
            .map_err(|error| error.to_string())?;
        window
            .set_size(tauri::LogicalSize::new(1000.0 + index as f64 * 20.0, 600.0))
            .map_err(|error| error.to_string())?;
        if index == 4 {
            window.hide().map_err(|error| error.to_string())?;
            // Visibility requests are queued; wait for hide before remapping.
            for _ in 0..100 {
                if !window.is_visible().map_err(|error| error.to_string())? {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            window.show().map_err(|error| error.to_string())?;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let paint_opacity = if legacy { 1.0 } else { opacity };
        let expected = format!("rgba(239, 241, 245, {paint_opacity})");
        let mut measured = (0, 0);
        for _ in 0..100 {
            let actual = javascript(
                &window,
                "document.documentElement.style.getPropertyValue('--window-bg-primary')",
            )
            .await?;
            if actual == expected {
                measured =
                    painted_pixels(&window, (paint_opacity * 255.0_f64).round() as u8).await?;
                if measured.0 > 10_000 && measured.1 > 20 {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if measured.0 <= 10_000 || measured.1 <= 20 {
            return Err(format!(
                "GTK draw at opacity {opacity}: background/text pixels {measured:?}"
            ));
        }
        if legacy && opacity < 1.0 {
            let message = javascript(&window, "document.querySelector('[data-settings-section=\"theme\"] [role=\"alert\"]')?.textContent ?? ''").await?;
            if !message.contains("WEBKIT_DISABLE_") {
                return Err(format!(
                    "Missing renderer explanation in Settings: {message}"
                ));
            }
        }
        println!(
            "PASS actual GTK toplevel opacity={opacity}: background={} opaque-text={}",
            measured.0, measured.1
        );
    }
    if !legacy {
        scrolling(&window).await?;
    }
    Ok(())
}

#[test]
#[ignore = "requires a live composited GTK display; run alone in its own process"]
fn native_window_appearance() {
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
    let config = home.path().join("config");
    std::fs::create_dir_all(config.join("grafium")).unwrap();
    std::fs::write(config.join("grafium/theme.txt"), "auto").unwrap();
    let current = config.join("smplos/current");
    std::fs::create_dir_all(current.join("theme")).unwrap();
    std::fs::write(current.join("theme.name"), "catppuccin-latte").unwrap();
    let colors = current.join("theme/colors.toml");
    std::fs::write(&colors, "popup_opacity = 0.9").unwrap();
    let graph = grafium_core::Graph::open(&home.path().join("graph")).unwrap();
    let document = (0..50).map(|index| format!(
        "- ## Heading {index}\n- A synthetic paragraph {index} for native scroll repaint coverage, with **bold text** and an ordinary nested outline.\n"
    )).collect::<String>();
    std::fs::write(graph.pages_dir.join("Scrolling fixture.md"), document).unwrap();
    graph.reindex_all().unwrap();
    crate::webkit_renderer::configure();
    gtk::glib::set_prgname(Some("grafium"));
    let mut context = tauri::generate_context!();
    assert!(
        context.config().app.windows[0].transparent,
        "Linux config must be compiled in"
    );
    context.config_mut().app.windows[0].title = "Grafium isolated appearance test".into();
    let outcome = Arc::new(Mutex::new(None));
    let result = outcome.clone();
    let app = tauri::Builder::default()
        .any_thread()
        .manage(super::startup::StartupWindow::default())
        .manage(crate::AppState {
            graph: Arc::new(Mutex::new(graph)),
            watcher: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            super::theme::get_app_theme,
            super::theme::get_system_appearance,
            super::startup::reveal_startup_window,
            super::layout::get_layout_preferences,
            super::graph::get_graph_info,
            super::pages::get_page,
            super::pages::list_pages,
            super::pages::list_page_summaries,
            super::pages::count_pages,
            super::pages::get_parent_page,
            super::pages::get_child_pages,
            super::blocks::list_blocks,
            super::links::get_backlinks,
            super::favorites::list_favorites,
            super::favorites::record_page_open,
        ])
        .setup(move |app| {
            let window = app.get_webview_window("main").unwrap();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let tested = exercise(window, colors).await;
                *result.lock().unwrap() = Some(tested);
                handle.exit(0);
            });
            Ok(())
        })
        .build(context)
        .unwrap();
    assert_eq!(app.run_return(|_, _| {}), 0);
    outcome
        .lock()
        .unwrap()
        .take()
        .expect("native test did not finish")
        .unwrap();
}
