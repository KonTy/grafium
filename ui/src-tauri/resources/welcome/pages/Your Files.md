Grafium keeps notes as local Markdown and indexes them in SQLite for search, links, tasks, and queries. You can inspect and edit the Markdown with another editor.

- `pages/` holds ordinary pages; slash namespaces map to nested folders.
- `journals/` holds daily pages.
- `assets/` holds shared media, including this graph's original orbit SVG.
- Graph metadata holds the index and state that does not live in plain note text.

## Pages that are not files

Not every page in this graph has a file behind it. Writing `[[Orbits]]` or `#astronomy` creates the page immediately so the link resolves and backlinks work, but nothing is written to `pages/` until the page has content. These are **placeholders**: real entries in the index, with real backlinks, and no `.md` file on disk.

This is why a page can appear in **All Pages** and in the namespace tree, yet be missing when you look in the folder with another editor. Nothing is lost — there was never a file.

**All Pages** has a filter for exactly this question:

- **All** — everything, placeholders included. The default.
- **Files** — only pages with a `.md` file on disk. Use this when you want the list to match what a file manager or `git status` shows.
- **Placeholders** — only pages that a link or tag named and nobody has written. A useful to-do list: each one is somewhere you meant to say something.

The filter applies to list and tree view alike, and is remembered per graph. Note that **Placeholders** empties the namespace tree of most of its structure, and **Files** empties the tag tree almost entirely — tag pages are usually the placeholders.

The file watcher notices external Markdown changes and updates the index. Avoid editing the same page simultaneously in two editors. Watching local edits and synchronizing a graph are separate features.

## Sync to another location

Grafium supports **filesystem** and **WebDAV** sync targets. Configure the target in **Settings**, where **Sync Now** lets you request a sync. The sync monitor starts with the app and can automatically sync a configured target when it becomes available. This is not continuous collaborative editing.

A WebDAV target involves a remote server: choose one you trust with your notes and configure its access carefully.

Sync is not a substitute for an independent backup. Check the destination and the result before relying on another copy, especially before changing targets or moving a graph.

## Back up more than the text

Keep a separate backup of the whole graph, including assets and metadata. Markdown alone does not preserve every review, preference, or index-backed state. App-level configuration may live outside the graph; record your graph locations and settings too. For example, the saved theme lives in `grafium/theme.txt` under the platform's configuration directory, not inside the graph.

Before a bulk import, move, or cleanup, make a backup and verify that you can read it. A second copy on the same disk is not protection against losing that disk.

The Welcome seed only populates an empty default graph. It does not refresh older tutorials or replace edited sample notes. Use [[Create Your Own Graph]] for a separate workspace, and [[Imports And Media]] to bring files into it.
