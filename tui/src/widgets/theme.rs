//! Central "use the terminal's own colors" policy.
//!
//! Every style used across the TUI is built from this module, and every
//! style here is composed only from text **modifiers** (bold/italic/
//! underline/dim/reversed) — never an explicit `Color`. That means the app
//! never overrides the user's terminal foreground/background palette; it
//! only ever emphasizes text relative to whatever colors are already in
//! effect. If a new widget needs a style, it should reuse a function here
//! rather than inventing a new `Style::default().fg(...)` somewhere else.

use ratatui::style::{Modifier, Style};

/// Border/title style for whichever panel currently has keyboard focus.
pub fn focused() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// Border/title style for panels that are visible but not focused.
pub fn unfocused() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// Markdown heading text (level 1 gets an extra underline).
pub fn heading(top_level: bool) -> Style {
    let base = Style::default().add_modifier(Modifier::BOLD);
    if top_level {
        base.add_modifier(Modifier::UNDERLINED)
    } else {
        base
    }
}

/// Inline code / fenced code block text.
pub fn code() -> Style {
    Style::default().add_modifier(Modifier::ITALIC)
}

/// Secondary/decorative text (bullets, help hints, borders that shouldn't
/// draw the eye).
pub fn muted() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// The marker that highlights the currently-selected row in a list/document
/// without needing a background color — swaps whatever fg/bg the terminal
/// already has.
pub fn selected() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}
