use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
enum ThemeFile {
    Missing,
    Content(String),
    Failed(String),
}

impl ThemeFile {
    fn read(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => Self::Content(content),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::Missing,
            Err(error) => Self::Failed(format!("{}: {error}", path.display())),
        }
    }
}

/// Content, rather than metadata, catches same-name palette edits and atomic
/// directory/symlink replacement. Missing files are a real (opaque) state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThemeSnapshot {
    name: ThemeFile,
    colors: ThemeFile,
}

impl ThemeSnapshot {
    pub(crate) fn read(current: &Path) -> Self {
        let read = || {
            let root = fs::canonicalize(current).unwrap_or_else(|_| current.to_path_buf());
            Self {
                name: ThemeFile::read(&root.join("theme.name")),
                colors: ThemeFile::read(&root.join("theme/colors.toml")),
            }
        };
        let mut previous = read();
        // Do not publish a name/palette pair observed halfway through a switch.
        for _ in 0..3 {
            let next = read();
            if next == previous {
                return next;
            }
            previous = next;
        }
        let error = ThemeFile::Failed("smplOS appearance changed while being read".into());
        Self {
            name: error.clone(),
            colors: error,
        }
    }

    pub(crate) fn theme_name(&self) -> Option<String> {
        match &self.name {
            ThemeFile::Content(name) if !name.trim().is_empty() => Some(name.trim().to_owned()),
            _ => None,
        }
    }

    fn opacity(&self) -> Result<f64, String> {
        match &self.colors {
            ThemeFile::Content(content) => background_opacity(content),
            ThemeFile::Missing => Ok(1.0),
            ThemeFile::Failed(error) => Err(error.clone()),
        }
    }

    pub(crate) fn log_diagnostics(&self) {
        if let ThemeFile::Failed(error) = &self.name {
            tracing::warn!("Could not read smplOS theme name: {error}");
        }
        if let Err(error) = self.opacity() {
            tracing::warn!("Could not read smplOS background opacity; using opaque: {error}");
        }
    }
}

pub(crate) fn smplos_current_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|path| path.join("smplos/current"))
}

fn flat_value(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if let Some(quote) = raw.chars().next().filter(|ch| *ch == '"' || *ch == '\'') {
        let mut escaped = false;
        for (offset, ch) in raw[1..].char_indices() {
            if ch == quote && !escaped {
                let end = offset + 1;
                let suffix = raw[end + 1..].trim();
                if !suffix.is_empty() && !suffix.starts_with('#') {
                    return Err("unexpected text after quoted value".into());
                }
                return if quote == '"' {
                    serde_json::from_str(&raw[..=end]).map_err(|error| error.to_string())
                } else {
                    Ok(raw[1..end].to_owned())
                };
            }
            escaped = quote == '"' && ch == '\\' && !escaped;
        }
        Err("unterminated quoted value".into())
    } else {
        let value = raw.split('#').next().unwrap_or_default().trim();
        if value.is_empty() {
            Err("empty value".into())
        } else {
            Ok(value.to_owned())
        }
    }
}

fn flat_values(content: &str) -> HashMap<String, Result<String, String>> {
    let mut values = HashMap::new();
    for line in content.lines().map(str::trim) {
        if line.starts_with('[') {
            break; // The smplOS contract contains top-level keys, not table keys.
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().trim_matches(['"', '\'']).to_owned();
            let value = if values.contains_key(&key) {
                Err(format!("duplicate key {key}"))
            } else {
                flat_value(value)
            };
            values.insert(key, value);
        }
    }
    values
}

fn background_opacity(content: &str) -> Result<f64, String> {
    let values = flat_values(content);
    let selected = values
        .get("app_background_opacity")
        .map(|value| ("app_background_opacity", value))
        .or_else(|| {
            values
                .get("popup_opacity")
                .map(|value| ("popup_opacity", value))
        });
    let Some((key, value)) = selected else {
        return Ok(1.0);
    };
    let value = value.as_ref().map_err(|error| format!("{key}: {error}"))?;
    let opacity = value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && (0.0..=1.0).contains(value));
    opacity.ok_or_else(|| format!("{key} must be a finite decimal in [0, 1]"))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemAppearance {
    theme_name: Option<String>,
    background_opacity: f64,
    native_transparency: bool,
}

/// GTK objects must never cross threads. Commands await the main-loop result,
/// rather than blocking the loop on a synchronous channel receive.
pub(crate) async fn native_transparency(window: &tauri::WebviewWindow) -> bool {
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::*;
        use tauri::Manager;

        let configured = window
            .app_handle()
            .config()
            .app
            .windows
            .iter()
            .any(|config| config.label == window.label() && config.transparent);
        if !configured {
            return false;
        }
        let window_on_main = window.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        if let Err(error) = window.run_on_main_thread(move || {
            let supported = window_on_main.gtk_window().ok().is_some_and(|native| {
                let Some(screen) = WidgetExt::screen(&native) else {
                    return false;
                };
                let rgba = screen.rgba_visual();
                let display_type = screen.display().type_().name();
                let backend_supported = match display_type {
                    "GdkWaylandDisplay" => true,
                    "GdkX11Display" => screen.is_composited(),
                    _ => false,
                };
                native.is_app_paintable()
                    && rgba.is_some()
                    && native.visual() == rgba
                    && backend_supported
            });
            let _ = send.send(supported);
        }) {
            tracing::warn!("Could not check native transparency: {error}");
            return false;
        }
        receive.await.unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = window;
        false
    }
}

#[tauri::command(rename_all = "camelCase")]
pub async fn get_system_appearance(window: tauri::WebviewWindow) -> SystemAppearance {
    let snapshot = smplos_current_dir().map(|path| ThemeSnapshot::read(&path));
    if let Some(snapshot) = &snapshot {
        snapshot.log_diagnostics();
    }
    SystemAppearance {
        theme_name: snapshot.as_ref().and_then(ThemeSnapshot::theme_name),
        background_opacity: snapshot
            .as_ref()
            .map_or(1.0, |snapshot| snapshot.opacity().unwrap_or(1.0)),
        native_transparency: native_transparency(&window).await,
    }
}

/// Get the current smplos theme name from ~/.config/smplos/current/theme.name
#[tauri::command(rename_all = "camelCase")]
pub fn get_smplos_theme() -> Result<Option<String>, String> {
    let Some(current) = smplos_current_dir() else {
        return Ok(None);
    };
    let snapshot = ThemeSnapshot::read(&current);
    match &snapshot.name {
        ThemeFile::Failed(error) => Err(error.clone()),
        _ => Ok(snapshot.theme_name()),
    }
}

/// Read colors.toml from a specific smplos theme (or current)
#[tauri::command(rename_all = "camelCase")]
pub fn get_smplos_theme_colors() -> Result<HashMap<String, String>, String> {
    let Some(current) = smplos_current_dir() else {
        return Ok(HashMap::new());
    };
    match ThemeSnapshot::read(&current).colors {
        ThemeFile::Missing => Ok(HashMap::new()),
        ThemeFile::Failed(error) => Err(error),
        ThemeFile::Content(content) => flat_values(&content)
            .into_iter()
            .map(|(key, value)| value.map(|value| (key, value)))
            .collect(),
    }
}

/// Get/set the user's preferred theme for Grafium (stored in app config)
#[tauri::command(rename_all = "camelCase")]
pub fn get_app_theme() -> Result<String, String> {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("grafium");

    let path = config_dir.join("theme.txt");
    if path.exists() {
        fs::read_to_string(&path)
            .map(|s| s.trim().to_string())
            .map_err(|e| e.to_string())
    } else {
        // First install: GitHub Light on desktop, OLED on Android.
        #[cfg(target_os = "android")]
        {
            Ok("oled".to_string())
        }
        #[cfg(not(target_os = "android"))]
        {
            Ok("github".to_string())
        }
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_app_theme(theme_id: String) -> Result<(), String> {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("grafium");

    fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    let path = config_dir.join("theme.txt");
    fs::write(&path, &theme_id).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(".theme-fixture-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap()
    }

    fn write_theme(current: &Path, opacity: &str) {
        fs::create_dir_all(current.join("theme")).unwrap();
        fs::write(current.join("theme.name"), "fixture-theme\n").unwrap();
        fs::write(
            current.join("theme/colors.toml"),
            format!("app_background_opacity = {opacity}\n"),
        )
        .unwrap();
    }

    #[test]
    fn opacity_accepts_decimal_boundaries_quotes_whitespace_and_comments() {
        for (input, expected) in [
            ("app_background_opacity = \"0.85\"", 0.85),
            (" \tapp_background_opacity\t= 0.625 # comment", 0.625),
            ("app_background_opacity = '0.45' # comment", 0.45),
            ("app_background_opacity = \"0\" # comment", 0.0),
            ("app_background_opacity = 1", 1.0),
            ("app_background_opacity = 0.0", 0.0),
            ("app_background_opacity = \"1.0\"", 1.0),
        ] {
            assert_eq!(background_opacity(input).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn opacity_falls_back_only_when_app_key_is_absent() {
        assert_eq!(background_opacity("").unwrap(), 1.0);
        assert_eq!(
            background_opacity("background = \"#123456\" # color").unwrap(),
            1.0
        );
        assert_eq!(
            background_opacity("popup_opacity = \"0.7\" # popup").unwrap(),
            0.7
        );
        assert_eq!(
            background_opacity("popup_opacity = 0.4\napp_background_opacity = \"0.8\"").unwrap(),
            0.8
        );
        assert_eq!(
            background_opacity("popup_opacity = 0.4\napp_background_opacity = 1").unwrap(),
            1.0
        );
        assert_eq!(
            background_opacity("popup_opacity = 0.4\napp_background_opacity = 0").unwrap(),
            0.0
        );
        assert_eq!(
            flat_values("background = \"#123456\" # color")["background"]
                .as_ref()
                .unwrap(),
            "#123456"
        );
    }

    #[test]
    fn present_invalid_opacity_is_opaque_never_popup_fallback() {
        for invalid in [
            "\"oops\"",
            "\"\"",
            "",
            "\"NaN\"",
            "nan",
            "NaN",
            "inf",
            "-inf",
            "\"Infinity\"",
            "\"0.9px\"",
            "-0.1",
            "1.01",
            "1e999",
            "true",
            "\"0.7\" trailing",
            "\"0.7",
            "\"0.7#comment\"",
            "[0.7]",
            "0x1",
        ] {
            let content = format!("popup_opacity = 0.2\napp_background_opacity = {invalid}");
            assert!(background_opacity(&content).is_err(), "{invalid}");
            let snapshot = ThemeSnapshot {
                name: ThemeFile::Missing,
                colors: ThemeFile::Content(content),
            };
            assert_eq!(snapshot.opacity().unwrap_or(1.0), 1.0, "{invalid}");
        }
        assert!(background_opacity("popup_opacity = \"invalid\"").is_err());
        assert!(
            background_opacity("app_background_opacity = 0.5\napp_background_opacity = 0.8")
                .is_err()
        );
    }

    #[test]
    fn snapshot_observes_missing_created_same_name_edit_removed_and_restored() {
        let fixture = fixture();
        let current = fixture.path().join("current");
        let missing = ThemeSnapshot::read(&current);
        assert_eq!(missing.theme_name(), None);
        assert_eq!(missing.opacity().unwrap(), 1.0);

        write_theme(&current, "\"0.8\"");
        let created = ThemeSnapshot::read(&current);
        assert_ne!(created, missing);
        assert_eq!(created, ThemeSnapshot::read(&current));
        assert_eq!(created.theme_name().as_deref(), Some("fixture-theme"));
        assert_eq!(created.opacity().unwrap(), 0.8);

        write_theme(&current, "\"0.3\"");
        let edited = ThemeSnapshot::read(&current);
        assert_ne!(created, edited);
        assert_eq!(created.theme_name(), edited.theme_name());
        assert_eq!(edited.opacity().unwrap(), 0.3);

        fs::remove_file(current.join("theme/colors.toml")).unwrap();
        let removed = ThemeSnapshot::read(&current);
        assert_ne!(removed, edited);
        assert_eq!(removed.opacity().unwrap(), 1.0);
        fs::remove_file(current.join("theme.name")).unwrap();
        assert_eq!(ThemeSnapshot::read(&current), missing);
        write_theme(&current, "\"0.3\"");
        assert_eq!(ThemeSnapshot::read(&current), edited);
    }

    #[test]
    fn snapshot_observes_directory_replacement_and_non_opacity_palette_changes() {
        let fixture = fixture();
        let current = fixture.path().join("current");
        let replacement = fixture.path().join("replacement");
        write_theme(&current, "\"0.8\"");
        write_theme(&replacement, "\"0.5\"");
        let before = ThemeSnapshot::read(&current);
        fs::rename(&current, fixture.path().join("old")).unwrap();
        fs::rename(&replacement, &current).unwrap();
        let after = ThemeSnapshot::read(&current);
        assert_ne!(before, after);
        assert_eq!(after.opacity().unwrap(), 0.5);
        fs::write(
            current.join("theme/colors.toml"),
            "app_background_opacity = \"0.5\"\nbackground = \"#abcdef\"",
        )
        .unwrap();
        let palette = ThemeSnapshot::read(&current);
        assert_ne!(after, palette);
        assert_eq!(after.opacity().unwrap(), palette.opacity().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_follows_atomic_current_and_theme_symlink_replacements() {
        use std::os::unix::fs::symlink;
        let fixture = fixture();
        let first = fixture.path().join("first");
        let second = fixture.path().join("second");
        let current = fixture.path().join("current");
        write_theme(&first, "\"0.8\"");
        write_theme(&second, "\"0.2\"");
        symlink(&first, &current).unwrap();
        let before = ThemeSnapshot::read(&current);
        let next = fixture.path().join("next");
        symlink(&second, &next).unwrap();
        fs::rename(&next, &current).unwrap();
        let after = ThemeSnapshot::read(&current);
        assert_ne!(before, after);
        assert_eq!(after.opacity().unwrap(), 0.2);

        fs::rename(second.join("theme"), second.join("saved-theme")).unwrap();
        symlink(first.join("theme"), second.join("theme")).unwrap();
        assert_eq!(ThemeSnapshot::read(&current).opacity().unwrap(), 0.8);
        symlink(second.join("saved-theme"), second.join("next-theme")).unwrap();
        fs::rename(second.join("next-theme"), second.join("theme")).unwrap();
        assert_eq!(ThemeSnapshot::read(&current), after);
    }

    #[test]
    fn read_failures_are_stable_explicit_and_opaque_until_recovery() {
        let fixture = fixture();
        let current = fixture.path().join("current");
        fs::create_dir_all(current.join("theme/colors.toml")).unwrap();
        let failed = ThemeSnapshot::read(&current);
        assert_eq!(failed, ThemeSnapshot::read(&current));
        assert!(failed.opacity().is_err());
        assert_eq!(failed.opacity().unwrap_or(1.0), 1.0);
        fs::remove_dir(current.join("theme/colors.toml")).unwrap();
        write_theme(&current, "\"0.4\"");
        let recovered = ThemeSnapshot::read(&current);
        assert_ne!(failed, recovered);
        assert_eq!(recovered.opacity().unwrap(), 0.4);
    }

    #[test]
    fn appearance_serialization_matches_frontend_contract() {
        let value = serde_json::to_value(SystemAppearance {
            theme_name: None,
            background_opacity: 0.75,
            native_transparency: false,
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "themeName": null, "backgroundOpacity": 0.75, "nativeTransparency": false,
            })
        );
    }

    #[test]
    fn linux_window_override_preserves_every_base_window_field() {
        let base: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let linux: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.linux.conf.json")).unwrap();
        let mut windows = linux["app"]["windows"].clone();
        for window in windows.as_array_mut().unwrap() {
            assert_eq!(
                window.as_object_mut().unwrap().remove("transparent"),
                Some(serde_json::json!(true))
            );
        }
        assert_eq!(windows, base["app"]["windows"]);
    }
}
