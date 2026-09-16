use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LayoutPreferences {
    sidebar_visible: bool,
    wide_mode: bool,
}

impl Default for LayoutPreferences {
    fn default() -> Self {
        Self {
            sidebar_visible: true,
            wide_mode: true,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayoutPreferencePatch {
    sidebar_visible: Option<bool>,
    wide_mode: Option<bool>,
}

fn config_dir() -> Result<PathBuf, String> {
    dirs::config_dir()
        .map(|dir| dir.join("grafium"))
        .ok_or_else(|| "Could not locate the app configuration directory.".to_string())
}

fn read_preferences(dir: &Path) -> Result<LayoutPreferences, String> {
    match std::fs::read(dir.join("layout.json")) {
        Ok(bytes) => {
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|err| format!("Could not read layout preferences: {err}"))?;
            if !value.is_object() {
                return Err("Layout preferences must be a JSON object".into());
            }
            return serde_json::from_value(value)
                .map_err(|err| format!("Could not read layout preferences: {err}"));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("Could not read layout preferences: {err}")),
    }
    let mut preferences = LayoutPreferences::default();
    match std::fs::read(dir.join("sidebar-visible.json")) {
        Ok(bytes) => {
            preferences.sidebar_visible = serde_json::from_slice(&bytes)
                .map_err(|err| format!("Could not read previous left menu preference: {err}"))?;
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(format!(
                "Could not read previous left menu preference: {err}"
            ))
        }
    }
    Ok(preferences)
}

fn write_preferences(dir: &Path, patch: LayoutPreferencePatch) -> Result<(), String> {
    // Merge under one lock so separate UI changes cannot overwrite each other.
    let _guard = WRITE_LOCK.lock().map_err(|err| err.to_string())?;
    let mut preferences = read_preferences(dir)?;
    if let Some(visible) = patch.sidebar_visible {
        preferences.sidebar_visible = visible;
    }
    if let Some(wide) = patch.wide_mode {
        preferences.wide_mode = wide;
    }
    let bytes = serde_json::to_vec(&preferences).map_err(|err| err.to_string())?;
    grafium_core::fsutil::atomic_write(&dir.join("layout.json"), &bytes)
        .map_err(|err| format!("Could not save layout preferences: {err}"))
}

#[tauri::command]
pub fn get_layout_preferences() -> Result<LayoutPreferences, String> {
    let preferences = read_preferences(&config_dir()?)?;
    tracing::info!(
        sidebar_visible = preferences.sidebar_visible,
        wide_mode = preferences.wide_mode,
        "Restoring saved layout"
    );
    Ok(preferences)
}

#[tauri::command]
pub fn set_layout_preferences(preferences: LayoutPreferencePatch) -> Result<(), String> {
    write_preferences(&config_dir()?, preferences)
}

#[tauri::command]
pub fn get_sidebar_visibility() -> Result<bool, String> {
    Ok(read_preferences(&config_dir()?)?.sidebar_visible)
}

#[tauri::command]
pub fn set_sidebar_visibility(visible: bool) -> Result<(), String> {
    write_preferences(
        &config_dir()?,
        LayoutPreferencePatch {
            sidebar_visible: Some(visible),
            ..Default::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_launch_shows_menu_without_writing_preferences() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_preferences(dir.path()).unwrap(),
            LayoutPreferences::default()
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn each_choice_survives_restart_without_overwriting_the_other() {
        let dir = tempfile::tempdir().unwrap();
        for visible in [false, true] {
            write_preferences(
                dir.path(),
                LayoutPreferencePatch {
                    sidebar_visible: Some(visible),
                    ..Default::default()
                },
            )
            .unwrap();
            for wide in [false, true, false] {
                write_preferences(
                    dir.path(),
                    LayoutPreferencePatch {
                        wide_mode: Some(wide),
                        ..Default::default()
                    },
                )
                .unwrap();
                assert_eq!(
                    read_preferences(dir.path()).unwrap(),
                    LayoutPreferences {
                        sidebar_visible: visible,
                        wide_mode: wide
                    }
                );
            }
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn old_sidebar_choices_migrate_without_resetting_closed_menus() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("sidebar-visible.json");
        std::fs::write(&legacy, "false").unwrap();
        assert!(!read_preferences(dir.path()).unwrap().sidebar_visible);
        write_preferences(
            dir.path(),
            LayoutPreferencePatch {
                wide_mode: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            read_preferences(dir.path()).unwrap(),
            LayoutPreferences {
                sidebar_visible: false,
                wide_mode: false
            }
        );
        std::fs::write(&legacy, "true").unwrap();
        assert!(
            !read_preferences(dir.path()).unwrap().sidebar_visible,
            "the unified record takes precedence after migration"
        );
    }

    #[test]
    fn invalid_preferences_are_reported_without_overwriting_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("layout.json");
        for invalid in ["", "null", "[]", "broken json", r#"{"wideMode":"false"}"#] {
            std::fs::write(&path, invalid).unwrap();
            assert!(read_preferences(dir.path()).is_err());
            assert!(write_preferences(dir.path(), LayoutPreferencePatch::default()).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
        }
    }

    #[test]
    fn partial_records_keep_safe_defaults_and_unknown_patch_keys_are_rejected() {
        let preferences: LayoutPreferences = serde_json::from_str(r#"{"wideMode":false}"#).unwrap();
        assert!(preferences.sidebar_visible);
        assert!(!preferences.wide_mode);
        assert!(serde_json::from_str::<LayoutPreferencePatch>(r#"{"wide":false}"#).is_err());
    }

    #[test]
    fn read_and_write_errors_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-directory");
        std::fs::write(&path, "occupied").unwrap();
        assert!(read_preferences(&path).is_err());
        assert!(write_preferences(&path, LayoutPreferencePatch::default()).is_err());
    }
}
