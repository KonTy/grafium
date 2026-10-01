const HELP_PAGES: &[(&str, &str)] = &[
    (
        "general",
        include_str!("../../resources/welcome/pages/Help - Grafium Guide.md"),
    ),
    (
        "editor",
        include_str!("../../resources/welcome/pages/Help - Editor.md"),
    ),
    (
        "journal",
        include_str!("../../resources/welcome/pages/Help - Journal Guide.md"),
    ),
    (
        "graph",
        include_str!("../../resources/welcome/pages/Help - Graph.md"),
    ),
    (
        "flashcards",
        include_str!("../../resources/welcome/pages/Help - Flashcards.md"),
    ),
    (
        "tasks",
        include_str!("../../resources/welcome/pages/Help - Tasks.md"),
    ),
    (
        "studies",
        include_str!("../../resources/welcome/pages/Help - Studies.md"),
    ),
    (
        "chat",
        include_str!("../../resources/welcome/pages/Help - Chat.md"),
    ),
    (
        "settings",
        include_str!("../../resources/welcome/pages/Help - Settings.md"),
    ),
    (
        "ai",
        include_str!("../../resources/welcome/pages/AI Setup And Privacy.md"),
    ),
    (
        "sync",
        include_str!("../../resources/welcome/pages/Help - Sync.md"),
    ),
    (
        "search",
        include_str!("../../resources/welcome/pages/Help - Search.md"),
    ),
    (
        "books",
        include_str!("../../resources/welcome/pages/Help - Books.md"),
    ),
    (
        "reader",
        include_str!("../../resources/welcome/pages/Help - Private Reader.md"),
    ),
];

#[tauri::command(rename_all = "camelCase")]
pub fn help_get_page(context: String) -> Result<String, String> {
    HELP_PAGES
        .iter()
        .find(|(name, _)| *name == context)
        .map(|(_, content)| (*content).to_string())
        .ok_or_else(|| format!("Unknown help context: {context}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn private_reader_help_explains_privacy_and_device_limits() {
        let page = super::help_get_page("reader".into()).unwrap();
        assert!(page.contains("not copied into your graph"));
        assert!(page.contains("[[Book title]]"));
        assert!(page.contains("unverified"));
        assert!(page.contains("screen-content"));
    }

    #[test]
    fn books_help_distinguishes_originals_from_conversion() {
        let page = super::help_get_page("books".into()).unwrap().to_lowercase();
        assert!(page.contains("original"));
        assert!(page.contains("convert to editable markdown"));
        assert!(page.contains("jsonld"));
        assert!(page.contains("unchanged"));
        assert!(page.contains("index"));
    }

    #[test]
    fn ai_help_includes_runtime_recovery_controls() {
        let page = super::help_get_page("ai".into()).unwrap();
        assert!(page.contains("Allow one GPU attempt"));
        assert!(page.contains("cgroup"));
    }
}
