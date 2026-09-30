use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{State, WebviewWindow};
use tauri::Manager;

/// Startups slower than this leave a phase report in `startup-slow.log` in the
/// app data directory, so a stall that can't be reproduced on demand still
/// says where the time went.
const SLOW_STARTUP: Duration = Duration::from_secs(3);
const MAX_SLOW_LOG_BYTES: u64 = 64 * 1024;

static PHASES: OnceLock<(Instant, Mutex<Vec<(&'static str, Duration)>>)> = OnceLock::new();

/// Record that a startup phase finished. The first call starts the clock.
pub fn mark(phase: &'static str) {
    let (started, phases) = PHASES.get_or_init(|| (Instant::now(), Mutex::new(Vec::new())));
    if let Ok(mut phases) = phases.lock() {
        phases.push((phase, started.elapsed()));
    }
}

fn phase_report() -> Option<(Duration, String)> {
    let (started, phases) = PHASES.get()?;
    let total = started.elapsed();
    let phases = phases.lock().ok()?;
    Some((total, format_phases(&phases, total)))
}

fn format_phases(phases: &[(&str, Duration)], total: Duration) -> String {
    let mut previous = Duration::ZERO;
    let mut out = String::new();
    for (phase, at) in phases {
        out.push_str(&format!(
            "{phase} +{}ms (at {}ms); ",
            at.saturating_sub(previous).as_millis(),
            at.as_millis()
        ));
        previous = *at;
    }
    out.push_str(&format!("window shown at {}ms", total.as_millis()));
    out
}

fn report_startup(app_data: Option<&Path>, how: &str) {
    let Some((total, report)) = phase_report() else {
        return;
    };
    if total < SLOW_STARTUP {
        tracing::info!("Startup timing ({how}): {report}");
        return;
    }
    tracing::warn!("Slow startup ({how}): {report}");
    let Some(dir) = app_data else {
        return;
    };
    let path = dir.join("startup-slow.log");
    let truncate = std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_SLOW_LOG_BYTES);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(!truncate)
        .truncate(truncate)
        .open(&path);
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let line = format!("unix={unix} {how}: {report}\n");
    if let Err(error) = file.and_then(|mut f| f.write_all(line.as_bytes())) {
        tracing::warn!("Could not record slow startup in {}: {error}", path.display());
    }
}

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
        report_startup(window.app_handle().path().app_data_dir().ok().as_deref(), "ready");
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
            let shown = show(&window, [30, 30, 46]);
            report_startup(app.path().app_data_dir().ok().as_deref(), "fallback");
            shown
        });
        if let Err(error) = result {
            tracing::error!("Could not reveal startup recovery window: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{format_phases, StartupWindow};
    use std::time::Duration;

    #[test]
    fn phase_report_shows_each_phase_duration() {
        let phases = [
            ("graph opened", Duration::from_millis(120)),
            ("setup finished", Duration::from_millis(12_400)),
        ];
        assert_eq!(
            format_phases(&phases, Duration::from_millis(13_000)),
            "graph opened +120ms (at 120ms); setup finished +12280ms (at 12400ms); window shown at 13000ms"
        );
    }
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
