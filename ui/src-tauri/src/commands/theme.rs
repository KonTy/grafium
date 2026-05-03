use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Get the current smplos theme name from ~/.config/smplos/current/theme.name
#[tauri::command(rename_all = "camelCase")]
pub fn get_smplos_theme() -> Result<Option<String>, String> {
    let path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("smplos/current/theme.name");

    if path.exists() {
        let name = fs::read_to_string(&path)
            .map_err(|e| e.to_string())?
            .trim()
            .to_string();
        Ok(Some(name))
    } else {
        Ok(None)
    }
}

/// Read colors.toml from a specific smplos theme (or current)
#[tauri::command(rename_all = "camelCase")]
pub fn get_smplos_theme_colors() -> Result<HashMap<String, String>, String> {
    let path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("smplos/current/theme/colors.toml");

    if !path.exists() {
        return Ok(HashMap::new());
    }

    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut colors: HashMap<String, String> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_string();
            let value = value.trim().trim_matches('"').to_string();
            colors.insert(key, value);
        }
    }

    Ok(colors)
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
        // Default: auto (follow smplos)
        Ok("auto".to_string())
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
