<div align="center">

# Grafium

**A fast, local-first personal knowledge base built on Rust and SQLite.**

Your notes live as plain Markdown files on disk. Grafium indexes them into a SQLite database for instant search, backlinks, queries, and structured knowledge management — without ever locking you into a proprietary format.

</div>

---

## Features

### Block-Based Outliner
Every page is a tree of blocks. Indent, reorder, collapse, and reference individual blocks across your entire knowledge base. Each block can hold Markdown, tasks, math (KaTeX), or code.

### Bidirectional Links & Graph
Connect ideas with `[[page links]]`, `#tags`, and `((block references))`. Backlinks are computed automatically — every mention is tracked and queryable.

### Hierarchical Pages
Organize pages into topic trees using slash-separated titles: `projects/grafium/roadmap` automatically nests under `projects/grafium`. Navigate the hierarchy from the sidebar or breadcrumbs.

### Journal
Daily notes are created automatically. Open the app and start writing — today's journal is always ready. Older entries scroll infinitely below.

### Raw SQL Queries
Embed live SQL queries directly in your pages using fenced code blocks. Grafium exposes read-only access to the underlying SQLite index, so you can write:

~~~markdown
```query
SELECT title, updated_at FROM pages WHERE is_journal = 0 ORDER BY updated_at DESC LIMIT 10
```
~~~

Results render inline as tables. Query pages, blocks, links, tasks, flashcards — anything in the index.

### Tasks
Mark blocks as `TODO`, `DOING`, `DONE`, or `CANCELED`. Attach `SCHEDULED` and `DEADLINE` dates. Query them across your entire graph to build dashboards and agendas.

### Flashcards (Spaced Repetition)
Turn any block into a flashcard. Grafium uses the SM-2 algorithm to schedule reviews at optimal intervals. Study directly inside the app.

### Full-Text Search
Powered by SQLite FTS5 with Porter stemming. Search across all content instantly — results are ranked by relevance and returned in milliseconds.

### Audio Notes
Attach audio recordings to pages with transcript storage for searchable voice notes.

### Live File Watcher
Edit your `.md` files in any external editor. Grafium detects changes on disk and re-indexes automatically — no manual sync needed.

---

## Performance

Speed is a core design goal, not an afterthought.

| Aspect | Implementation |
|---|---|
| **Database** | SQLite in WAL mode with aggressive indexing |
| **Connection pooling** | r2d2 pool for concurrent read/write access |
| **Search** | FTS5 full-text search with Porter stemming |
| **Incremental indexing** | Only changed blocks are re-parsed and updated |
| **Debounced saves** | 300ms batching to avoid disk thrashing during rapid edits |
| **Startup** | Near-instant — opens the SQLite index, skips reindex if already populated |
| **Binary size** | Single native binary via Tauri — no Electron, no bundled Chromium |

---

## Tech Stack

- **Core engine** — Rust (`grafium-core`): parser, database, models
- **Database** — SQLite with WAL, FTS5, JSON1 extensions
- **Frontend** — Svelte 5 with CodeMirror 6 editor
- **Desktop shell** — Tauri 2 (native webview, ~10MB binary)
- **Markdown** — Marked parser, Turndown for HTML→MD, KaTeX for math

---

## Prerequisites

### All platforms
- Rust (via [rustup](https://rustup.rs))
- Node.js + npm

### Linux (Arch)
```bash
sudo pacman -S webkit2gtk-4.1 libsoup3 gtk3 librsvg
```

### Linux (Ubuntu/Debian)
```bash
sudo apt install libwebkit2gtk-4.1-dev libsoup-3.0-dev libgtk-3-dev librsvg2-dev
```

## Build & Run

```bash
# Install JS dependencies
cd ui && npm install

# Run in development (desktop)
cd ui && npm run tauri dev

# Build for production
cd ui && npm run tauri build

# Run Rust tests
cargo test -p grafium-core
```

## License

MIT
