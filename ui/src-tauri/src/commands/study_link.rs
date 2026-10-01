#[tauri::command(rename_all = "camelCase")]
pub async fn study_link_title(url: String) -> Result<String, String> {
    grafium_core::study_link::fetch_study_link_title(&url)
        .await
        .map_err(|error| error.to_string())
}
