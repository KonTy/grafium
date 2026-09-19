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
        "chat",
        include_str!("../../resources/welcome/pages/Help - Chat.md"),
    ),
    (
        "settings",
        include_str!("../../resources/welcome/pages/Help - Settings.md"),
    ),
    (
        "sync",
        include_str!("../../resources/welcome/pages/Help - Sync.md"),
    ),
    (
        "search",
        include_str!("../../resources/welcome/pages/Help - Search.md"),
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
