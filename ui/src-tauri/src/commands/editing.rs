//! Native editing commands.
//!
//! The context menu's Paste must behave exactly like Ctrl+V. Under Tauri's
//! default WebKitGTK settings the page may not read the clipboard at all
//! (`navigator.clipboard.read*` and `execCommand("paste")` are both refused),
//! and widening that permission for every script in the page is not worth one
//! menu item. WebKit's own paste command needs no such grant: it reads the
//! clipboard itself and delivers an ordinary `paste` event to the focused
//! element, which is the path the editors already handle.

use serde::Serialize;
#[cfg(target_os = "linux")]
use tauri::WebviewWindow;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NativePasteOutcome {
    /// WebKit was asked to paste into the focused element.
    Requested,
    /// This platform has no native paste path; the caller falls back to the
    /// web clipboard API. Only the non-Linux build returns it, but the
    /// frontend reads it as part of the contract.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Unsupported,
}

#[cfg(target_os = "linux")]
#[tauri::command]
pub async fn native_paste(window: WebviewWindow) -> Result<NativePasteOutcome, String> {
    use webkit2gtk::WebViewExt;

    window
        .with_webview(|webview| {
            // WebKitGTK's `WEBKIT_EDITING_COMMAND_PASTE`.
            webview.inner().execute_editing_command("Paste");
        })
        .map_err(|error| error.to_string())?;
    Ok(NativePasteOutcome::Requested)
}

#[cfg(not(target_os = "linux"))]
#[tauri::command]
pub async fn native_paste() -> Result<NativePasteOutcome, String> {
    Ok(NativePasteOutcome::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::NativePasteOutcome;

    #[test]
    fn outcomes_serialize_as_the_strings_the_frontend_expects() {
        assert_eq!(
            serde_json::to_string(&NativePasteOutcome::Requested).unwrap(),
            "\"requested\""
        );
        assert_eq!(
            serde_json::to_string(&NativePasteOutcome::Unsupported).unwrap(),
            "\"unsupported\""
        );
    }
}
