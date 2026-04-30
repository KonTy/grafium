# PKM - Personal Knowledge Management

A blazing-fast, cross-platform Logseq-style personal knowledge management app built with Rust + Tauri 2 + React.

## Architecture

```
/core            → Rust engine (library crate: pkm-core)
/core/src        → Rust modules: db, models, parser, query, error
/ui              → Tauri 2 + React frontend
/ui/src          → React components, hooks, state, pages
/ui/src-tauri    → Tauri Rust backend (bridges core → UI)
```

## Performance Design

- **SQLite with WAL mode** + aggressive indexing for millions of records
- **Connection pooling** (r2d2) for concurrent access
- **FTS5 full-text search** with Porter stemming
- **Incremental parsing** — only re-indexes changed blocks
- **Virtual lists** (react-virtuoso) — renders only visible items
- **Debounced saves** — 300ms delay to batch rapid edits

## Features

- Block-based outliner editor (Logseq-style)
- [[Page links]], #tags, ((block references))
- Tasks: TODO/DOING/DONE/CANCELED with SCHEDULED/DEADLINE dates
- Flashcards with SM-2 spaced repetition
- Full-text search across all content
- Query engine: `{{query (and [[Project]] (task TODO))}}`
- Favorites & Recent pages
- Hierarchical topic tree from page titles (tech/rust/async)
- Journal pages (daily notes)
- Dark theme with CSS variables

## Prerequisites

### All platforms
- Rust (rustup)
- Node.js + npm

### Linux (Arch)
```bash
sudo pacman -S webkit2gtk-4.1 libsoup3 gtk3 librsvg
```

### Linux (Ubuntu/Debian)
```bash
sudo apt install libwebkit2gtk-4.1-dev libsoup-3.0-dev libgtk-3-dev librsvg2-dev
```

### Android
- Android SDK + NDK
- Tauri 2 Android prerequisites

## Build & Run

```bash
# Install JS dependencies
cd ui && npm install

# Run in development (desktop)
cd ui && npm run tauri dev

# Build for production
cd ui && npm run tauri build

# Run Rust tests
cargo test -p pkm-core
```

## Module Structure

### Core Engine (`/core`)
| Module | Purpose |
|--------|---------|
| `db/` | SQLite operations, schema, CRUD for all entities |
| `models.rs` | Data structs: Page, Block, Task, Flashcard, etc. |
| `parser/` | Markdown parser (Logseq-style) + link extraction |
| `query/` | Query AST, parser, and SQL executor |
| `error.rs` | Error types |

### UI (`/ui/src`)
| Module | Purpose |
|--------|---------|
| `components/` | Layout, Sidebar, BlockEditor, SearchModal |
| `pages/` | JournalPage, PageView, FlashcardsPage, GraphPage, AllPages |
| `store/` | Zustand global state |
| `lib/api.ts` | Tauri command bindings (typed) |
| `styles/` | CSS variables + component styles |

## Targets

- Linux (native WebKitGTK)
- Windows (native WebView2)
- Android (Tauri 2 mobile)
- macOS / iOS (planned)
