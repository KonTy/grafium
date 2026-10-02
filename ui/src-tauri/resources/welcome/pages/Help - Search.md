# Help - Search

Press **Ctrl+K** / **Cmd+K**, or use the title-bar Search button, to open
**Search your graph**. These open the same dialog, whether the sidebars are
expanded, collapsed, or hidden.

Search page titles to navigate quickly, or search block content for exact notes.
Use **Up/Down** to browse results, **Enter** to open one, and **Escape** to close
the dialog. Ordinary text search needs no AI; **AI Search** adds semantic matches
when an embedding model is configured.

The left panel is for navigation, favorites, and recent pages, not a separate
search interface. **Ctrl+B** / **Cmd+B** expands or focuses its navigation;
press it again while focused there to collapse it. The old sidebar-search
shortcut **Ctrl+Shift+K** / **Cmd+Shift+K** has been removed.

**Ctrl+F** / **Cmd+F** focuses an existing filter in views such as All Pages,
Graph, Library, or Settings. Those filters narrow the current view; they do not open
another graph-search dialog.

## Files or placeholders in All Pages

**All Pages** lists every page, including ones that only exist because a `[[link]]` or `#tag` named them. Those have no Markdown file on disk yet.

Use the **All / Files / Placeholders** control to choose which you see. **Files** matches what you would find in the folder; **Placeholders** is a list of pages you meant to write. The choice is remembered per graph.

Deleting a source removes its searchable content, but its title may remain as
a placeholder if another document still links to it. Shared tags and other
documents' references are preserved. See [[Your Files]] for deletion, index
cleanup, and the difference between removing a source and erasing saved quotations.

In the namespace tree, Grafium keeps **Books**, **ImportedMedia**, and **Reading Notes** at the top in that order, whether you sort by name or recent activity. Their book, media, and note icons distinguish these app-managed folders from ordinary folders. The tag tree still follows the selected sort normally.
