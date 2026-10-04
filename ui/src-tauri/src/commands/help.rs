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
    (
        "library",
        include_str!("../../resources/welcome/pages/Help - Library.md"),
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
    fn library_help_explains_separate_ownership_and_reference_plans() {
        let page = super::help_get_page("library".into()).unwrap();
        assert!(page.contains("stable Library item and bookmark IDs"));
        assert!(page.contains("not another copy"));
        assert!(page.contains("g l"));
    }

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
        assert!(page.contains("Try faster mode"));
        assert!(page.contains("cgroup"));
    }

    #[test]
    fn library_help_explains_disconnected_locations() {
        let page = super::help_get_page("library".into()).unwrap();
        assert!(page.contains("Library locations"));
        assert!(page.contains("**Disconnected**"));
        assert!(page.contains("needs no relinking"));
        assert!(page.contains("**Change folder…**"));
        assert!(page.contains("never touches the files"));
    }

    #[test]
    fn general_help_explains_the_jobs_bell() {
        let page = super::help_get_page("general".into()).unwrap();
        assert!(page.contains("Click the bell in the title bar"));
        assert!(page.contains("completed or failed since you last opened Jobs"));
        assert!(page.contains("**New**"));
    }

    #[test]
    fn chat_help_lists_the_composer_shortcuts() {
        let page = super::help_get_page("chat".into()).unwrap();
        assert!(page.contains("**Alt+N** selects the next available notes context"));
        assert!(page.contains("**Alt+A**"));
        assert!(page.contains("add Shift to go back"));
    }
}
