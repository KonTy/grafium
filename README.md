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
Embed live SQL queries directly in your pages using `{{query ...}}` blocks. Grafium exposes read-only access to the underlying SQLite index, so you can build dashboards and views right inside your notes.

Results render inline as tables. Query pages, blocks, links, tasks, flashcards — anything in the index. Query blocks automatically exclude themselves and other query blocks from results.

#### Query Examples

**All open tasks across the graph:**
```
{{query SELECT p.title AS page, b.content AS task FROM blocks b JOIN pages p ON b.page_id = p.id WHERE b.content LIKE 'TODO %' OR b.content LIKE 'DOING %'}}
```

**Tasks scheduled this week:**
```
{{query SELECT b.content AS task, p.title AS page FROM blocks b JOIN pages p ON b.page_id = p.id JOIN tasks t ON t.block_id = b.id WHERE t.scheduled_date BETWEEN date('now') AND date('now', '+7 days')}}
```

**All YouTube videos in the graph:**
```
{{query SELECT p.title AS page, b.content AS video FROM blocks b JOIN pages p ON b.page_id = p.id WHERE b.content LIKE '%youtube.com%' OR b.content LIKE '%youtu.be%'}}
```

**Find a specific video (e.g. about drones):**
```
{{query SELECT p.title AS page, b.content AS video FROM blocks b JOIN pages p ON b.page_id = p.id WHERE b.content LIKE '%youtube.com%' AND b.content LIKE '%drone%'}}
```

**Recently modified pages (non-journal):**
```
{{query SELECT title, datetime(updated_at, 'unixepoch') AS modified FROM pages WHERE is_journal = 0 ORDER BY updated_at DESC LIMIT 10}}
```

**Pages with the most blocks:**
```
{{query SELECT p.title, COUNT(*) AS block_count FROM blocks b JOIN pages p ON b.page_id = p.id GROUP BY b.page_id ORDER BY block_count DESC LIMIT 10}}
```

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

# Run frontend unit tests
cd ui && npm test
```

### Browser UI tests

`npm test` and `svelte-check` both pass straight through a class of failure
that only appears once components run together — an effect loop that freezes a
whole screen, a filter that finds matches and leaves them hidden, a search that
quietly collapses folders you had opened. Each of those shipped, and each was
caught here and nowhere else.

```bash
cd ui
npx playwright install chromium   # once
npm run test:ui
```

The run starts its own dev server and stops it again, and stubs the Tauri IPC,
so it needs no graph and touches no data. It is kept out of `npm test` because
it needs a browser.

## License

MIT
