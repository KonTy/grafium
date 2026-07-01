# MASTER BUILD SPEC FOR GRAPH-STYLE PKM APP
# STACK: RUST + SQLITE + TAURI + REACT + CSS
# STORAGE: MARKDOWN FILES + FULL SQLITE INDEXING
# FEATURES: HANDWRITING, AUDIO, AI, FLASHCARDS, GRAPH VIEW, FAVORITES, RECENT, HIERARCHICAL TOPICS, SCHEDULED/DEADLINE, DAILY QUERIES, GRAPH IMPORTER
# THIS SPEC IS FINAL AND MUST BE FOLLOWED EXACTLY BY AIDER

Aider MUST follow phases in order. No redesigning architecture. No skipping or merging phases.

================================================================================
PHASE 0 — REPO INITIALIZATION
================================================================================
0.1 Monorepo structure:

/core            → Rust engine (library crate)
/core/src        → Rust modules
/ui              → Tauri + React frontend
/ui/src          → React components, hooks, state
/shared          → Shared schemas, docs
/docs            → Architecture docs
/scripts         → Build scripts

0.2 Initialize:
- Rust workspace
- /core as Rust library crate
- Tauri app in /ui
- React + TypeScript in /ui
- Tooling: Rustfmt, Clippy, ESLint, Prettier

0.3 README with architecture overview and build instructions.

================================================================================
PHASE 1 — RUST CORE: DATA MODEL + SQLITE SCHEMA
================================================================================
Goal: Foundational schema for pages, blocks, links, flashcards, tasks, audio, handwriting.

1.1 Enable SQLite with:
- WAL
- FTS5
- JSON1
- Optional vector extension

1.2 Tables:

TABLE pages:
- id TEXT PRIMARY KEY
- title TEXT UNIQUE
- file_path TEXT
- created_at INTEGER
- updated_at INTEGER
- is_journal INTEGER
- meta JSON

TABLE blocks:
- id TEXT PRIMARY KEY
- page_id TEXT
- parent_id TEXT
- order_index INTEGER
- content TEXT
- block_type TEXT   -- "text", "handwriting", "audio", "mixed", "flashcard"
- properties JSON   -- includes task, scheduling, tags, etc.
- created_at INTEGER
- updated_at INTEGER

TABLE links:
- from_block_id TEXT
- to_page_id TEXT
- type TEXT          -- "page", "tag", "topic"

TABLE fts_blocks (FTS5):
- block_id
- content

TABLE handwriting_strokes:
- id TEXT PRIMARY KEY
- block_id TEXT
- strokes BLOB or TEXT
- created_at INTEGER

TABLE audio_notes:
- id TEXT PRIMARY KEY
- block_id TEXT
- audio_path TEXT
- duration_ms INTEGER
- created_at INTEGER

TABLE audio_transcripts:
- id TEXT PRIMARY KEY
- audio_id TEXT
- transcript TEXT
- is_relevant INTEGER
- meta JSON

TABLE flashcards:
- id TEXT PRIMARY KEY
- block_id TEXT
- front TEXT
- back TEXT
- tags JSON
- created_at INTEGER
- updated_at INTEGER
- last_reviewed_at INTEGER
- next_review_at INTEGER
- ease_factor REAL
- interval_days INTEGER
- review_count INTEGER

TABLE favorites:
- id TEXT PRIMARY KEY
- page_id TEXT
- created_at INTEGER

TABLE recent_pages:
- id TEXT PRIMARY KEY
- page_id TEXT
- last_opened_at INTEGER

TABLE tasks:
- id TEXT PRIMARY KEY
- block_id TEXT
- state TEXT          -- "TODO", "DOING", "DONE", "CANCELED", etc.
- scheduled_date TEXT -- ISO date
- deadline_date TEXT  -- ISO date
- created_at INTEGER
- updated_at INTEGER

1.3 Rust structs:
- Page, Block, Link
- HandwritingStroke
- AudioNote, AudioTranscript
- Flashcard
- Task
- QueryResult

1.4 CRUD functions for all entities.

1.5 Backlink generation:
- Parse [[Page]] and #tags.
- Insert into links.
- Update FTS.

1.6 Topic hierarchy:
- Parse titles like [[tech/bing/cool]].
- Store topic path in properties:
  - properties.topic_path = ["tech", "bing", "cool"].
- Use this for tree view in “All pages”.

================================================================================
PHASE 2 — RUST CORE: MARKDOWN PARSER + FILE WATCHER
================================================================================
Goal: Markdown as primary storage, SQLite as index.

2.1 Markdown parser:
- Parse blocks by indentation.
- Parse block properties (key:: value).
- Parse block IDs (id:: uuid).
- Parse tasks:
  - TODO, DOING, DONE, CANCELED, etc.
- Parse SCHEDULED and DEADLINE:
  - SCHEDULED: <2024-01-01>
  - DEADLINE: <2024-01-01>
- Parse links:
  - [[Page]]
  - #tag
  - ((block-id))
- Parse flashcards:
  - #flashcard tag on block
  - Or :: syntax for front/back:
    - Front :: Back
- Parse queries:
  - {{query ...}}

2.2 Serializer:
- Blocks → Markdown file with properties and structure preserved.

2.3 File watcher:
- Watch /pages directory.
- On file change:
  - Parse Markdown.
  - Update pages, blocks, tasks, flashcards, links, FTS.
  - Incremental updates only.

2.4 Journal detection:
- Journal filenames like 2024_01_01.md → is_journal = 1.

================================================================================
PHASE 3 — RUST CORE: QUERY ENGINE
================================================================================
Goal: outline-style queries for tasks, pages, flashcards, etc.

3.1 Query language:
- {{query [[Page]]}}
- {{query "text search"}}
- {{query (and [[Project]] (task TODO))}}
- {{query (property key value)}}
- {{query (and (scheduled today) (task TODO))}}
- {{query (deadline before 2024-01-01)}}

3.2 AST:
- QueryPage
- QueryText
- QueryAnd
- QueryOr
- QueryProperty
- QueryTaskState
- QueryScheduled
- QueryDeadline

3.3 Parser → AST.

3.4 AST → SQL over blocks, tasks, pages, links, FTS.

3.5 Return list of blocks with metadata.

================================================================================
PHASE 4 — RUST CORE: TAURI COMMAND API
================================================================================
Expose engine to UI:

Pages:
- list_pages()
- get_page(title or id)
- create_page(title)
- update_page_meta(id, meta)
- delete_page(id)

Blocks:
- list_blocks(page_id)
- create_block(page_id, parent_id, order_index, content, block_type, properties)
- update_block(id, content, properties)
- delete_block(id)
- reorder_blocks(page_id, new_order)

Links:
- get_backlinks(page_id)

Search + queries:
- run_query(query_string)
- search_fts(query_string)

Handwriting:
- save_handwriting_strokes(block_id, strokes_encoded)
- get_handwriting_strokes(block_id)

Audio:
- register_audio_note(block_id, audio_path, duration_ms)
- get_audio_note(block_id)
- save_audio_transcript(audio_id, transcript, is_relevant, meta)
- get_audio_transcripts(block_id)

Flashcards:
- list_flashcards(filter: tags, due_only)
- get_flashcard_for_block(block_id)
- update_flashcard_review(id, ease_factor, next_review_at, interval_days, review_count)

Favorites + recent:
- add_favorite(page_id)
- remove_favorite(page_id)
- list_favorites()
- record_page_open(page_id)
- list_recent_pages(limit)

Tasks:
- list_tasks(filter: state, scheduled, deadline)
- update_task_state(block_id, new_state)
- update_task_dates(block_id, scheduled_date, deadline_date)

Import:
- import_page(file_path)
- import_directory(dir_path)

All commands return JSON-safe types.

================================================================================
PHASE 5 — UI FOUNDATION: LAYOUT + ROUTING + STATE
================================================================================
5.1 Routing:
- / → Today’s journal
- /journal/:date
- /page/:title
- /flashcards
- /graph
- /settings

5.2 Layout:
- Left sidebar:
  - Favorites
  - Recent
  - Flashcards
  - Graph
  - All pages (tree view)
- Right sidebar:
  - Backlinks
  - Queries
  - Page metadata
- Center:
  - Block editor

5.3 Global state (Zustand or similar):
- currentPage
- currentBlocks
- sidebar state
- theme
- settings
- flashcard session state

5.4 Keyboard shortcuts:
- g j → journal
- g h → home
- g f → flashcards
- g g → graph
- t l → toggle left sidebar
- t r → toggle right sidebar
- / → slash command menu

================================================================================
PHASE 6 — UI: TEXT BLOCK EDITOR
================================================================================
6.1 Inline Markdown editing:
- Focused: raw Markdown.
- Blurred: rendered Markdown.

6.2 Markdown renderer:
- bold, italic, highlight
- [[links]]
- ((block refs))
- #tags
- code blocks
- checkboxes
- headings
- properties (key:: value)

6.3 Editing behavior:
- Enter → new block
- Shift+Enter → newline
- Tab / Shift+Tab → indent/outdent
- Arrow navigation
- Backspace merge

6.4 Visual structure:
- Vertical lines for hierarchy.
- Collapse/expand children.

================================================================================
PHASE 7 — HANDWRITING BLOCKS (FIRST-CLASS)
================================================================================
7.1 Block type: "handwriting".

7.2 Canvas component:
- Pen, highlighter, eraser.
- Stylus vs touch.
- Undo/redo.
- Zoom/pan.

7.3 Stroke storage:
- Encode strokes as JSON/binary.
- Save via Tauri.

7.4 Inline behavior:
- Handwriting blocks behave like text blocks in hierarchy.

7.5 Slash command:
- /handwriting.

================================================================================
PHASE 8 — MIXED BLOCKS (TEXT + HANDWRITING)
================================================================================
8.1 Block type: "mixed".

8.2 UI:
- Text + canvas in same block.

8.3 Inline small canvases (optional).

================================================================================
PHASE 9 — AUDIO BLOCKS (FIRST-CLASS)
================================================================================
9.1 Block type: "audio".

9.2 Recording:
- /audio inserts audio block.
- Start/pause/stop.
- Save audio via Tauri.
- Register in SQLite.

9.3 Playback:
- Inline player.
- Speed control.

================================================================================
PHASE 10 — AI TRANSCRIPTION + RELEVANCE FILTERING
================================================================================
10.1 Rust AI adapter:
- transcribe_audio(audio_path) -> transcript.
- classify_segments(transcript, context) -> segments with is_relevant.

10.2 Transcription flow:
- User clicks "Transcribe".
- Transcript stored in audio_transcripts.
- Optionally split into child blocks.

10.3 Relevance:
- Highlight relevant text.
- Dim or hide unrelated.
- Toggle: show all / show relevant.

10.4 Search:
- Include transcripts in FTS.

================================================================================
PHASE 11 — AI FOR TEXT + HANDWRITING
================================================================================
11.1 Text AI:
- summarize(text)
- rewrite(text, style)

11.2 Handwriting:
- recognize_handwriting(strokes) -> text.

11.3 Semantic search:
- embed(text) -> vector (optional).

================================================================================
PHASE 12 — FLASHCARDS MODULE
================================================================================
Goal: outline-style flashcards created from blocks, reviewed in a dedicated view.

12.1 Flashcard creation:
- Any block with #flashcard tag becomes a flashcard.
- Or use :: syntax:
  - Front :: Back
- Importer detects:
  - content before :: → front
  - content after :: → back
- Store in flashcards table:
  - front, back, tags, block_id.

12.2 Flashcard types:
- Basic: front/back.
- Cloze (optional): use {{c1::text}} syntax.

12.3 Flashcard view (/flashcards):
- Modes:
  - Due cards (based on next_review_at).
  - All cards.
  - Filter by tag/topic.

12.4 Review flow:
- Show front.
- On reveal, show back.
- User rates:
  - Again / Hard / Good / Easy.
- Update:
  - ease_factor
  - interval_days
  - next_review_at
  - review_count

12.5 Integration with journal:
- Users can write notes in journal with #flashcard.
- Indexer auto-creates/updates flashcards.

================================================================================
PHASE 13 — TASKS, SCHEDULED, DEADLINE, TODO QUERIES
================================================================================
Goal: outline-style task management with /SCHEDULED, /DEADLINE, and queries.

13.1 Task detection:
- Blocks starting with:
  - TODO, DOING, DONE, CANCELED, etc.
- Store in tasks table.

13.2 /SCHEDULED command:
- Slash command /SCHEDULED.
- Opens date picker.
- Inserts:
  - SCHEDULED: <YYYY-MM-DD>.
- Updates tasks.scheduled_date.

13.3 /DEADLINE command:
- Slash command /DEADLINE.
- Opens date picker.
- Inserts:
  - DEADLINE: <YYYY-MM-DD>.
- Updates tasks.deadline_date.

13.4 Task queries:
- {{query (and (task TODO) (scheduled today))}}
- {{query (deadline before 2024-01-01)}}
- {{query (and [[Project]] (task TODO))}}

13.5 Daily TODO sections:
- Today’s journal page can have:
  - A readonly or template section where TODO queries are rendered.
  - Example:
    - Under a heading "Today’s Tasks" render:
      - All TODO with scheduled today.
- Allow user to configure:
  - Where queries appear (top, bottom, under date).

================================================================================
PHASE 14 — DAILY PAGE TEMPLATES + READONLY SECTIONS
================================================================================
14.1 Daily template:
- Define a template for journal pages:
  - Sections like:
    - "Morning"
    - "Tasks"
    - "Notes"
- Template stored in settings/meta.

14.2 Readonly sections:
- Some sections auto-populated by queries:
  - e.g., "Tasks" section shows TODO query.
- User cannot directly edit query results, only underlying tasks.

14.3 Template application:
- On new journal page:
  - Apply template.
  - Insert query blocks where configured.

================================================================================
PHASE 15 — QUERY BLOCKS (UI)
================================================================================
15.1 Detect {{query ...}} in blocks.

15.2 Send query to Rust.

15.3 Render results as block list.

15.4 Auto-refresh when underlying data changes.

================================================================================
PHASE 16 — THEMING ENGINE (CSS)
================================================================================
16.1 CSS variables:
- Colors, typography, spacing, block styles, sidebars.

16.2 Theme loader:
- Load theme.css and custom.css.
- Live switching.

16.3 Theme packs:
- themes/<name>/theme.css.

16.4 Stable CSS class hooks for all components.

================================================================================
PHASE 17 — PLUGIN SYSTEM
================================================================================
17.1 Plugin manifest:
- name, version, entry JS, optional CSS.

17.2 Sandbox:
- Isolated JS context.

17.3 Plugin API:
- registerCommand
- registerPanel
- registerSlashCommand
- injectCSS

17.4 Plugins can:
- Add sidebar panels.
- Add commands.
- Add slash commands.
- Add custom views (e.g., custom flashcard views, analytics).

================================================================================
PHASE 18 — GRAPH IMPORTER
================================================================================
18.1 Import Grafium directory:
- For each .md file:
  - Detect if journal or page.
  - Parse blocks, properties, tasks, flashcards, queries.
  - Preserve block IDs.
  - Preserve SCHEDULED/DEADLINE.
  - Preserve #flashcard tags.
  - Preserve [[topic/subtopic/subject]] paths.

18.2 Insert into SQLite:
- pages, blocks, tasks, flashcards, links, FTS.

18.3 Rebuild backlinks.

================================================================================
PHASE 19 — GRAPH VIEW + ANALYTICS
================================================================================
19.1 Graph view (/graph):
- Global graph:
  - Nodes = pages.
  - Edges = links.
- Per-topic graph:
  - Filter by topic path.
- Per-page graph:
  - Focus on one page and neighbors.

19.2 Implementation:
- Canvas/WebGL-based graph (e.g., force-directed).
- React as wrapper.

19.3 TODO analytics:
- Completion rate over time.
- Procrastination index:
  - e.g., tasks with scheduled date < completion date.
- Time-series charts for tasks, habits.

================================================================================
PHASE 20 — SIDEBAR MODULES: FAVORITES, RECENT, ALL PAGES TREE
================================================================================
20.1 Favorites:
- Star icon on page.
- Stored in favorites table.
- Sidebar section "Favorites" lists starred pages.

20.2 Recent:
- On page open, record in recent_pages.
- Sidebar section "Recent" shows last N pages.

20.3 All pages tree:
- Use topic_path from titles like [[tech/bing/cool]].
- Build tree:
  - tech
    - bing
      - cool
- Cap depth at 10 levels.
- Pages without topic path appear at root.

================================================================================
PHASE 21 — SEARCH, SETTINGS, POLISH
================================================================================
21.1 Global search:
- FTS over blocks + transcripts.

21.2 Settings:
- Theme selection.
- AI backend config.
- Flashcard settings.
- Daily template config.

21.3 Tests:
- Rust unit tests.
- Basic React tests.

21.4 Performance tuning.

================================================================================
END OF SPEC
================================================================================
