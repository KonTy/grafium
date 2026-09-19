use std::sync::Mutex;
use tauri::{State, WebviewWindow};
// Only `install_fallback` needs the `Manager` methods, and it is desktop-only.
#[cfg(desktop)]
use tauri::Manager;

#[derive(Default)]
pub struct StartupWindow(Mutex<bool>);

impl StartupWindow {
    fn reveal_once(&self, reveal: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        let mut shown = self.0.lock().map_err(|e| e.to_string())?;
        if !*shown {
            reveal()?;
            *shown = true;
        }
        Ok(())
    }
}

#[cfg(desktop)]
fn show(window: &WebviewWindow, background: [u8; 3]) -> Result<(), String> {
    let [r, g, b] = background;
    // Color both the native surface and WebKit's backing surface before mapping.
    if let Err(error) = window.set_background_color(Some(tauri::window::Color(r, g, b, 255))) {
        tracing::warn!("Could not set startup window background: {error}");
    }
    window.show().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reveal_startup_window(
    window: WebviewWindow,
    state: State<'_, StartupWindow>,
    background: [u8; 3],
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Only the main window can complete startup".into());
    }
    state.reveal_once(|| {
        #[cfg(desktop)]
        show(&window, background)?;
        #[cfg(mobile)]
        let _ = background;
        tracing::info!("Startup window ready");
        Ok(())
    })
}

#[cfg(desktop)]
pub fn install_fallback(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        let Some(window) = app.get_webview_window("main") else {
            return;
        };
        let result = app.state::<StartupWindow>().reveal_once(|| {
            tracing::warn!("Startup readiness timed out; showing the recovery window");
            if let Err(error) = window.eval(
                r#"const status = document.getElementById("grafium-startup-message");
                if (status) {
                  status.textContent = "Startup is taking longer than expected. You can retry loading Grafium.";
                  document.getElementById("grafium-startup-retry").hidden = false;
                  document.getElementById("grafium-startup").setAttribute("role", "alert");
                }"#,
            ) {
                tracing::warn!("Could not display startup recovery message: {error}");
            }
            show(&window, [30, 30, 46])
        });
        if let Err(error) = result {
            tracing::error!("Could not reveal startup recovery window: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::StartupWindow;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[test]
    fn ready_and_fallback_only_reveal_once() {
        let state = StartupWindow::default();
        let calls = AtomicUsize::new(0);
        for _ in 0..2 {
            state
                .reveal_once(|| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_show_leaves_recovery_available() {
        let state = StartupWindow::default();
        assert!(state.reveal_once(|| Err("window error".into())).is_err());
        let mut recovered = false;
        state
            .reveal_once(|| {
                recovered = true;
                Ok(())
            })
            .unwrap();
        assert!(recovered);
    }

    #[test]
    fn competing_readiness_and_timeout_do_not_show_twice() {
        let state = Arc::new(StartupWindow::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let state = state.clone();
                let calls = calls.clone();
                std::thread::spawn(move || {
                    state
                        .reveal_once(|| {
                            calls.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        })
                        .unwrap()
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
