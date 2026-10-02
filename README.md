<div align="center">

<img src="ui/src-tauri/icons/grafium-logo.svg" width="80" alt="Grafium logo">

# Grafium

**Write like an editor. Think like an outliner.**

A local-first notebook for ideas that deserve more than a place to sit.
Write, connect, plan, study, and explore your knowledge without giving up your files.

[Download](https://github.com/KonTy/grafium/releases/latest) ·
[Get started](#start-with-one-idea) ·
[Feature reference](#feature-reference) ·
[Build from source](#build-from-source)

</div>

![Grafium's Welcome graph: an editable, linked introduction to the notebook](docs/images/grafium-welcome.png)

*The screenshots in this README use only the included Welcome graph, never personal notes.*

## Start with one idea

Capture a thought in today's journal. Turn a phrase into a `[[page link]]`.
Make the next step a `TODO`, or a useful fact a `Question :: Answer` flashcard.
The same writing becomes something you can find, follow, act on, and remember.

Grafium keeps each block an **addressable piece of knowledge**. Nest an outline,
select groups of blocks, and edit Markdown in place: click into a block to work
with its source, then move away to read the rendered result. Block identities
remain preserved in SQLite. A page is not flattened into one giant database
record.

1. **Open Grafium.** A self-contained Welcome graph gives you real notes, links,
   tasks, tables, queries, and flashcards to explore. No AI setup is needed for the tour.
2. **Try something small.** Edit a sample block, follow a link, sort a table,
   or open the graph and take a flight.
3. **Make your own space.** Choose **New Graph** from the graph menu and select a
   folder. You can switch between graphs; the Welcome graph stays separate.

## See the connections, not just the dots

The **2D graph** makes densely connected groups easier to read, with well-connected
pages at their centers and longer bridges between communities. Search for a page,
inspect its neighborhood, or zoom out to the wider graph.

![The Welcome graph in 2D, with linked communities and readable page labels](docs/images/grafium-graph-2d.png)

The **3D graph** shows the same pages and accepted connections in depth. Solid
spheres, a field of varied stars, and distant galaxy images give your knowledge a
different sense of scale. Unlinked pages sit on an imaginary sphere rather than a
flat ring. Dates and journal connections remain visible by default.

**Space flight** is a guided flyover through the visible graph: follow links,
approach a named topic, orbit it, and continue. Namespace children become
satellites around their parent topics. Search first to choose a starting point;
**Stop flight** or **Escape** returns you to the original layout.

![A Space flight through the Welcome graph, with topic planets and an offline universe backdrop](docs/images/grafium-space-flight.png)

These are views of your knowledge, not changes to it. Decorative stars, galaxies,
planet rings, satellites, and flight-only hierarchy routes do not create notes or
rewrite saved links.

<details>
<summary>How the graph layout works</summary>

Both views share deterministic, weighted
[Louvain-style community detection](https://arxiv.org/abs/0803.0476), followed by a
custom community-level layout, hub-centered placement, and anchored refinement.
The separation of tight groups and longer bridges follows graph-readability
principles discussed in [ForceAtlas2](https://doi.org/10.1371/journal.pone.0098679);
Grafium does not implement the full ForceAtlas2 or Leiden algorithms.

Community colors represent link structure in the loaded graph, not AI-assigned
subjects. The result can change with local/global scope or the node limit.
Suggested links are separate from structural clustering and spring attraction.
The 2D view manages label collisions and prioritizes hubs; both views preserve
manual navigation and dragging.

Stars vary in size and brightness. Galaxy images are bundled for offline use and
credited in [the space asset attribution](ui/public/space/ATTRIBUTION.txt).
The backdrop appears in normal 3D viewing and flight, with a quieter treatment
behind the normal graph. It is decorative, not an astronomical map.

</details>

## Feature reference

Settings keeps explanations behind compact **?** help buttons. Click a button
for its topic, or use **F1** for the full contextual guide. Settings search
includes hidden guidance without expanding it; errors, current results, and
destructive-action confirmation warnings remain visible.

### Writing and reading

| Feature | What you can do |
| --- | --- |
| Block selection | Shift+Up/Down selects text first, then whole blocks at the edge, including across journal days. Reverse direction to shrink the selection; copy, cut, delete, and undo work on the group without merging its stored blocks. Deleting keeps the caret at the gap: start of the next block, end of the previous block if no next block remains, or an editable blank on an emptied page. |
| In-place Markdown | Edit a focused block as source and read it as rendered Markdown when you leave it. |
| Nested outlines | Indent, outdent, reorder, fold, and expand blocks and their children. |
| Bullet threading | Follow the active outline branch with connected guides and elbows. |
| Editing tools | Split and merge blocks, insert a newline within a block, undo/redo edits, and copy or paste outlines. |
| Formatting | Headings, emphasis, strikethrough, quotes, lists, inline code, and fenced code blocks. |
| Math | Write inline and display mathematics with KaTeX. |
| Sortable Markdown tables | Click a rendered column heading to sort ascending or descending, with numeric-aware ordering. The new order is saved back to Markdown. |
| Rich paste | Convert pasted HTML to Markdown and preserve supported structure and media. |
| Images, audio, and video | Keep graph assets alongside notes and render supported media inline, including on flashcards. |
| Commands and templates | Use the editor's command menus rather than memorizing every Markdown pattern. |
| Emoji and icon picker | Type `/emoji` for emoji, `/icon` for built-in symbolic icons, or `/em` for both; add search words and choose from the completions in either editor. |
| Portable icons | Emoji are stored as literal characters. Icon shortcodes such as `:icon-star:` remain readable in source and render as icons outside code. |
| Bionic reading | Press **Ctrl/Cmd+Alt+B** or use **B** in book controls to emphasize word beginnings in rendered notes and reflowable ebooks. It changes presentation, not source text or saved passage anchors, and leaves code and math alone. |
| Book navigation | Arrow icons, Left/Right, Page Up/Down, swipes, and the mouse wheel navigate books. Reflowable EPUB/FB2/MOBI/AZW3 offer remembered layouts and text size, responsive text, and real font scaling; PDFs retain their page layout. |
| Callouts | Insert styled notes, warnings, tips, and other supported callout templates. |
| Selection actions | Turn selected bullets into TODOs or tasks back into bullets, and make links from selected text. |
| Reading notes | Open the right panel's **Notes** tab, select a passage and choose **Use selection**, then write and **Save note**. Notes can also apply to the whole current page, book, or journal day. No AI is required. |
| Markdown-source annotations | Notes on Markdown pages are footnotes inside the source file, with hidden quote-anchor metadata. Copying the book's `.md` carries its annotations too; SQLite is only a rebuildable index. Click a footnote marker to open its note in the pane. Original-format books instead keep one adjacent JSON-LD annotation file. Ambiguous or missing passages remain flagged instead of being guessed. |

New blank Markdown-note drafts automatically attach selected words or whole
blocks; check the quote preview before saving. **Use selection** changes the
attachment explicitly after writing begins; **Make page-level note** removes it.
**Delete note…** opens a focused confirmation with a preview, without scrolling
the notes list. **More actions > Delete all notes on this page…** removes only the current source's annotations,
even in the All notes view. Source prose is preserved and unsaved note edits are
kept as new drafts.

Reading-note drafts survive navigation during the app session, but **Save note**
is required before closing the app. For Markdown sources, saving adds reference
markers and footnote definitions without rewriting the book's words. Original
book files are never rewritten by annotation saves. Conflicting external edits leave
your draft intact. Earlier standalone Markdown note files remain supported and
are not automatically moved or deleted. Your annotations are kept separate from
the book evidence used by AI research. A misplaced inline note can be reattached
within its source file; moving it to a different file is not yet supported.
Passage selection requires saved block IDs; imported files without them are not
guessed into a different block.

### Journals, calendars, and tasks

| Feature | What you can do |
| --- | --- |
| Daily journals | Start with today's page and scroll through older entries in one journal view. Arrow Down/Up continues into the adjacent day when the caret reaches the last/first line. |
| **Go to date calendar** | Open with **Ctrl/Cmd+G**, navigate with the keyboard, and jump to a past or future journal. Circled days contain notes; the selected day's page is opened or created for you. |
| Fast month/year navigation | Click the calendar's month or year heading to choose directly, page through years, or return with **Today**. |
| Journal shortcuts | Move to today's, tomorrow's, previous, or next journal without hunting through the page tree. |
| Task blocks | Keep `TODO`, `DOING`, `DONE`, and `CANCELED` items inside the notes that give them context. |
| Task dates | Set **SCHEDULED** and **DEADLINE** dates with a calendar picker. |
| Priorities and recurrence | Read task priorities and Org-style timestamp repeaters such as `.+1d`, `++1w`, and `+1m`; completing a repeating task advances its next occurrence. |
| Task-format compatibility | Read Logseq/Org tasks, Markdown checkboxes, and supported Obsidian Tasks date/priority metadata; write a consistent task representation. |
| Tasks workspace | Group and sort open work by date, page, or priority and jump back to its source note. |
| Completion history | Record completion timestamps in Markdown and browse completed tasks by day. |
| Activity and flow | Explore task-completion and note-edit heatmaps, completion pace, cycle/lead time, and on-time completion metrics. |
| Live task views | Gather work from across the graph into query-driven lists and dashboards. |
| Progressive journal loading | Older editors load near the viewport and stay mounted once opened, preserving ongoing edits and selection. |

### Connecting and finding

| Feature | What you can do |
| --- | --- |
| Page links | Connect ideas with `[[page names]]` and autocomplete existing titles. |
| Block references | Refer to an individual block with `((block-id))`, not just its containing page. |
| Tags | Add `#tags` and hierarchical tags to make ideas easier to gather. |
| Backlinks | See the notes that refer to the current page, with block-level context. |
| Reviewable link suggestions | Scan exact mentions or ask AI to find concept edges; **Link**, **Link all**, **Fix + link**, or dismiss with **Not an edge**, with undo available. |
| Consistent concept links | Generated summaries and accepted suggestions display concepts as `#multi_word_name`, stored as `[[Canonical page title|#multi_word_name]]`. The label does not rename the destination. Existing titles and approved aliases are reused; ambiguous candidates require a choice. Discovering or dismissing suggestions does not create pages. |
| Namespaces | Organize slash-separated titles such as `Projects/Observatory/Checklist` into a navigable tree. |
| Collections | Mark a page as a book or paper collection from its page menu; navigate its ordered linked members, or convert it back to a regular page. |
| All Pages | Browse the namespace hierarchy and find pages without remembering their exact location. |
| Files or placeholders | Filter All Pages, in list and tree view, to every page, only pages with a source file on disk (Markdown or an original book), or only the placeholders a `[[link]]` or `#tag` created but nobody has written. Remembered per graph. |
| Favorites and recent pages | Keep frequently used material close at hand and return to recent work. |
| Full-text search | Search indexed note content with SQLite FTS5 and ranked results. |
| Go to link | Press **Ctrl/Cmd+L**, or use the link icon beside the calendar in Journal's top toolbar, to browse all pages with an immediately focused fuzzy search. Desktop and Android keep both icons beside Search rather than in an extra row above the notes. Arrows browse; Enter opens; Escape cancels. |
| Global search | Search pages and note content in one **Ctrl/Cmd+K** dialog, also opened by the title-bar Search button. The left panel stays focused on navigation, favorites, and recent pages. Exact search works without AI; semantic search uses the configured embedding index. |
| View filters and history | Focus an available All Pages, Graph, or Settings filter with **Ctrl/Cmd+F**, and move backward or forward through navigation history. |
| 2D and 3D graphs | Explore global or local connections, search, pan, zoom, inspect, and drag nodes. |
| Community layouts | Distinguish dense groups from the bridges between them, using shared clustering in both views. |
| 3D flyovers | Take a **Space flight** through topics, or use manual camera navigation. |

### Learning and working with sources

| Feature | What you can do |
| --- | --- |
| Flashcards in your notes | Write `Question :: Answer` to make a reviewable card without maintaining a second copy. |
| Spaced repetition | Reveal answers and grade recall with **Again**, **Hard**, **Good**, or **Easy**; SM-2 schedules subsequent reviews. |
| Study topics | Use tags to study one topic or mix cards from across your notes. |
| Studies workspace | Curate books, graph pages, flashcard topics, audio/video, YouTube videos, and websites in one list. Search all local source types or paste a link in one picker; titles and types fill automatically, with one Add confirmation. Reuse saved topics or add a new one. Filter rows and time cards, and resume saved reading/playback positions. Reading and flashcards pause their clock after 90 seconds of inactivity; media follows active playback while Grafium is focused. Websites open externally with manual checkpoints and no automatic time tracking. |
| Library | A top-level private media shelf for local EPUBs, audiobooks, downloaded podcasts, videos, and supported YouTube/direct media links. Resume from a recent pile, favorite items, and revisit bookmarks. Media and automatic history stay outside graph search, sync, and AI context; Studies plans link by stable ID without duplicating playback state. Journal notes are explicit graph content. See [Library help](ui/src-tauri/resources/welcome/pages/Help%20-%20Library.md) and [private reader guidance](ui/src-tauri/resources/welcome/pages/Help%20-%20Private%20Reader.md) for backup, folder, and device limitations. |
| Rich flashcards | Include Markdown, mathematics, images, audio, and video in cards. |
| Anki import | Bring `.apkg` decks and supported media into Grafium's note-and-card workflow. |
| Original-book library | Copy EPUB, PDF, FB2, MOBI, or AZW3 into the graph, read the original under Books, and keep notes with Markdown bodies in one adjacent JSON-LD file (`1.epub` → `1.jsonld`). Source files remain unchanged. Extracted text feeds graph search and configured AI indexing; extraction warnings are shown explicitly. |
| Optional book conversion | Check **Convert to editable Markdown** to retain the existing folder/file conversion workflow for EPUB, PDF, HTML, Markdown, text, FB2, and additional formats through optional Calibre tools. Unchecked imports preserve originals. Existing converted books are not migrated. |
| Reading structure | Keep paragraphs nested under detected chapter and section headings, with referenced media where extraction is supported. |
| Distraction-free reading | Read with hidden controls; tap, press F8, or use the corner button to reveal them. F11 enters fullscreen, Escape exits, and horizontal swipes turn pages. Reflowable text follows Grafium's theme with real font scaling; image and fixed-layout artwork retain their colors. |
| Scanned PDF OCR | Extract text locally with Poppler and Tesseract; optional ImageMagick helps extract figure regions. |
| Audio/video processing | Turn supported media sources into notes with transcripts; local transcription needs its model and supporting tools. |
| Background jobs | Follow longer-running imports and processing, cancel supported running jobs, and clear finished entries without blocking the editor. |

Original-format books, metadata, and adjacent `.jsonld` notes live under `books/`.
Legacy standalone Markdown notes under `pages/Reading Notes/Books/` remain
supported without automatic migration. Back up all of them. New annotation media
paths are relative to the book folder; legacy Markdown paths keep their meaning.
Copy a book with its matching Grafium sidecar (`1.epub` and `1.jsonld`) and import
the book to carry annotations into another graph. Import validates the pair
before writing; reimport combines histories while preserving local edits,
deletions, and unresolved conflicts. Managed graph copies use
`books/<book-id>/original.<extension>` and `original.jsonld`.
Referenced note media must also be preserved separately at its relative paths.
Sync combines independent notes, but competing edits or deletion-versus-edit
candidates are shown distinctly for manual review in **Notes**. Choose a
candidate attachment (or a current selection/whole-book note), edit the merged
Markdown, and explicitly **Resolve with merged note**, or confirm deletion.
Stale resolution reloads candidates without discarding your session draft.
The JSON-LD uses Web Annotation concepts plus Grafium revision data; universal
cross-reader compatibility is not promised. The reader remembers your
place and saved notes can return to a passage. DRM-protected ebooks and
fixed-layout annotation parity are not supported. MOBI/AZW3 text indexing may
require Calibre; scanned PDF text search requires OCR. Reading an original does
not require an AI model, and does not guarantee complete text extraction.

The private library is a different workflow from original-book import: it does
not make graph copies or automatically index external books for AI. Its
application-private state and external sources need their own backups.
Android physical volume-key bookmarking is opt-in and **unverified on actual
locked/screen-off Samsung, Pixel, and Vivo devices**; an Accessibility permission
grant alone does not establish key delivery. Do not equate headphone play/pause
support with phone-volume-key compatibility. Local speech requires a compatible
installed runtime and licensed voice; missing prerequisites must not trigger a
cloud or system-default voice fallback.

### Live queries

Ask questions of the actual SQLite index from inside a note. `{{query ...}}`
blocks render results inline as tables, so a dashboard can live beside the work
it describes. Queries are read-only; query blocks exclude themselves and other
query blocks from content results. Use SQL `ORDER BY` for query-table sorting;
click-to-sort applies to ordinary Markdown tables.

**Open tasks, with their source pages:**

```text
{{query SELECT p.title AS page, b.content AS task FROM tasks t JOIN blocks b ON b.id = t.block_id JOIN pages p ON p.id = b.page_id WHERE t.state IN ('TODO', 'DOING') ORDER BY p.title, b.order_index}}
```

**Recently changed pages:**

```text
{{query SELECT title, datetime(updated_at, 'unixepoch') AS modified FROM pages WHERE is_journal = 0 ORDER BY updated_at DESC LIMIT 10}}
```

**Upcoming scheduled work:**

```text
{{query SELECT p.title AS page, b.content AS task, t.scheduled_date AS scheduled FROM tasks t JOIN blocks b ON b.id = t.block_id JOIN pages p ON p.id = b.page_id WHERE t.state IN ('TODO', 'DOING') AND t.scheduled_date BETWEEN date('now') AND date('now', '+7 days') ORDER BY t.scheduled_date}}
```

The Welcome graph includes working examples against its own sample notes.

### Optional AI and research

AI is an addition to the notebook, not a requirement for writing, linking,
searching, querying, graph exploration, or flashcard review.

| Feature | What you can do |
| --- | --- |
| One Chat, two placements | Use the same conversation interface in the main Chat view or beside your notes. The right panel has **Chat** and **Notes** tabs. Expanding a conversation keeps its identity, messages, draft, and active request rather than starting another chat. |
| Explicit context | Sidebar Chat starts with the current page, recognized book, or focused journal day. Choose a selected passage, a block and its children, an available heading-defined section, the whole book, or **My graph**. Main Chat starts with **No notes**, which skips note retrieval and excludes earlier note-backed turns from the model's conversation history. |
| Answer -- no web | Answer from the selected context and the configured model's knowledge without searching the internet. This is the default; asking to "verify" or "research" something does not silently enable browsing. |
| Web search | Grafium searches the web and supplies retrieved evidence to the configured model for an answer. Context and web permission are separate choices. |
| Deep web research | Grafium plans searches, reads sources, assesses gaps, refines queries, and synthesizes a cited answer over multiple rounds. This is an explicit mode, not a second checkbox or a promise to read an entire book. |
| Research controls | Configure search engines, source/round limits, and workflow prompts. |
| Source-specific conversations | Each source has its own thread and draft. Answers continue while the panel is hidden or another page is open. Requests retain their initiating context and mode. Returning restores that source's conversation; **New conversation** clears it and restores the default context and no-web mode. Switching graphs stops active requests. |
| Several conversations at once | Chat keeps a list of conversations rather than one box, with a switcher to move between them. Each is named from its opening question and then renamed by the model once there is an answer to summarize; a name you type yourself is never overwritten. Right-click to rename or delete. |
| Clearing chats out | One trash button covers both jobs: with nothing selected it deletes every chat, and with chats ticked it deletes only those. Hover a chat for its checkbox, shift-click for a range, **Escape** to clear. The confirmation names the exact scope and warns when a conversation being deleted is still mid-answer. |
| Conversations are stored, and stay on this machine | The 50 most recent conversations are saved in the graph's `.grafium` folder and reloaded on restart. Sync never collects them, so they do not travel to a phone, a USB stick, or a file server -- a conversation can quote notes the other end has no business receiving. |
| Conversation memory is recency-based | A conversation that still fits is sent in full. Once it does not, the last four turns are kept and everything older is cut to its first 220 characters and gathered into a recap, capped at about 4,000 characters and shrunk further when the prompt still does not fit. The recap is truncation, not a model-written summary, and old turns are not searchable by meaning -- unlike notes, there is no semantic recall over conversation history. The full transcript stays readable on screen; the limit is on what the model is sent. |
| Long-source research | Retrieve bounded excerpts instead of putting an entire book into the question. Prompt fitting accounts for history, model tokens, and answer space while preserving citation labels. Answers disclose partial coverage; targeted questions are supported, not a guaranteed exhaustive review of every chapter. |
| Provider choice | Configure local models, a self-hosted endpoint, or a supported cloud provider. The model connection is shown separately from context and web mode. |
| Model settings | Manage generation and embedding configuration separately; local models require suitable weights and hardware. |
| Embedded or server-based models | On desktop, use embedded llama.cpp with local GGUF files, Ollama, or an OpenAI-compatible endpoint; cloud options include OpenAI and Anthropic. |
| Page and selection analysis | Run knowledge analysis for the current page or selected blocks instead of processing everything. |
| Explicit writing tools | Summarize, find links, and open writing assistance from Chat's tools rather than separate competing conversation tabs. Asking a question never silently rewrites your notes; insertions and rewrites require their explicit actions. |
| Safe summary insertion | Generated summaries remain associated with their source page. **Insert into page** adds a linked block tree after the captured reading position, leaves original text unchanged, and records one undo action. It reuses existing concept pages, creates new concepts only on insertion, and reports ambiguous concepts left unlinked. Code, math, URLs, and existing links are protected. |
| Ask about long videos and notes | Block context searches the block and its children; page context searches only the selected page or journal day. Chat selects bounded, relevant excerpts from current text rather than pasting the whole transcript into the question. Keyword retrieval works immediately without an embedding index; available, current vector matches improve ranking. Full prompts, history, and answer reserves are fitted to the model context, using the native tokenizer for embedded models. Long sources may be only partially covered. |
| AI writing assessment | Open **Writing assistance** from Chat's tools or the command palette. **Analyze AI style** uses your connected model to explain formulaic writing patterns, with a subjective 0-100 style score or an inconclusive result and explicit coverage. This is not a validated authorship detector or a probability that AI wrote the text. |
| Natural rewriting | **Rewrite naturally** makes small wording edits in the selected block or page, including scientific and technical text. Grafium retains protected numbers, citations, and formatting instead of asking the model to reproduce them; this limits sentence rearrangement. Invalid proposals leave the affected wording unchanged and are reported by block and line; validated edits apply together. Block identities and unsupported blocks stay unchanged. Review meaning and scientific details before keeping changes. In journals, Page scope uses only the selected day. Rewrites are guarded against concurrent edits and recorded as one Ctrl+Z undo action; Ctrl+Y redoes them. No guarantee of lower scores from external detectors. |

**Grafium owns web access, not the model provider.** Web search and Deep web
research run Grafium's search and page-reading tools, then pass evidence through
the configured model's normal API. An OpenAI-compatible DGX Spark or another
self-hosted model server does not need internet access or provider-native
browsing. Grafium itself needs network access for web modes; it does not switch
models to obtain it.

**No web is not a privacy switch for the model connection.** An API-connected
model still receives your question and selected note evidence, even in Answer
mode. A configuration called "cloud" can point to a private server; it does not
mean that server can browse. Web modes send derived queries to external search
engines and contact websites. Model and media downloads also require network
access.

### Your workspace, your files

| Feature | What you can do |
| --- | --- |
| Multiple graphs | Create and switch between separate graph folders. |
| Portable Markdown | Keep page and journal content as ordinary `.md` files, with graph-relative assets. |
| Live file watching | Edit files externally and let Grafium reconcile the changes with its index. |
| Media import inbox | Media imported by URL is filed under `ImportedMedia/` so untriaged material collects in one place instead of scattering. Importing into today's journal is unaffected. |
| Filesystem and WebDAV sync | Configure USB drives, mounted network folders, or a WebDAV server such as Nextcloud; run **Sync Now** from Settings. |
| Target availability | Configured auto-sync targets synchronize when the native monitor detects that they have become available. |
| Sync reporting | See pushed/pulled files, deletions, conflicts, and errors. File sync is not simultaneous collaborative editing; review conflicts and keep backups. |
| Theme choice | Use built-in light, dark, and OLED themes, including GitHub Light and GitHub Dark, with vivid, contrast-tuned reds, blues, and complementary accents for headings, tags, and callouts. Matrix keeps its green-on-black character with more colorful highlights. |
| Background transparency | Auto follows recognized smplOS themes and their background opacity on supported Linux desktops, without fading text/icons or changing opacity on focus. Explicit Grafium palettes and unsupported desktops stay opaque; menus, paper/media, and the 3D space scene retain readability backgrounds. |
| Reading width | Adjust narrow-view padding as a percentage on each side; the default is **15% per side**. |
| Wide mode | Switch between full-width and narrow reading layouts; the selected mode is remembered across app launches. |
| Zen mode and panels | Hide distractions or toggle the left and right sidebars independently. The left menu starts open on desktop and remembers your open/closed choice across launches. |
| Findable settings | Filter labels and help text with literal search terms, and open categorized keyboard-shortcut help. |
| Maintenance tools | Manually re-index Markdown and every original book copied into the graph, retrying extraction while preserving identities, annotations, favorites, review progress, and handwriting recognition. Rebuild search data and queue vector refresh; inspect asset-cleanup candidates or preview task-completion backfills separately. |
| Recoverable asset cleanup | Deleted text/blocks move newly unreferenced attachments to persistent graph-local trash; Undo restores references and files. Settings offers manual cleanup plus selected/all restore or explicitly confirmed permanent deletion. Shared references are kept conservatively. |
| Responsive workspace | Use the desktop layout or Android's adapted editor controls. |
| Theme-aware startup | Apply the saved theme before showing the desktop window; defer optional screens and local embedding-model loading. |

Your notes are portable, but Markdown is not a complete backup of every piece of
application state. Preserve graph folders, assets, and application data when
backing up or moving installations, including review scheduling and other database
history. Keep credentials and private data out of public repositories.
Sync sends graph files to the destinations you configure; use destinations you trust.

**Asset Cleanup:** Save pending edits before scanning or confirming a move.
Deleting text or blocks automatically moves newly unreferenced attachments into
persistent graph-local trash; Undo restores references and files while the
attachments remain in trash. Whole-page asset cleanup uses the same trash,
without making page deletion itself undoable.
Automatic cleanup errors appear as graph-bound notifications without
discarding a successful text edit or its Undo history. Restoration errors
instead stop Undo so it can be retried after resolving the error.
AI chat messages, citations, and conversation context are not checked for page
or attachment ownership. Old chat links may stop opening after cleanup; save
an excerpt as a note if its attachments should be kept.
The scan checks indexed data references and saved graph text, including
Markdown, JSON-LD notes, and configuration; unsaved drafts are not covered. Conservative
filename matching can retain duplicates. The preview is bound to its graph, and
references and file contents are checked again before moving. Only confirmed
moves leave the preview; errors remain visible.
References inside binary archives or books are not inspected, so this is not an
exhaustive check of every possible reference.

Moved attachments retain their relative paths under a unique
`.grafium/asset-trash/` directory. Results show the absolute recovery directory.
In **Settings > Asset Cleanup > Asset trash**, choose **List trash** or
**Refresh trash** to see original paths, trash paths, and sizes. **Open containing
folder** opens that trash copy's folder in the default system file manager
(Linux uses the directory MIME handler), without restoring or deleting it.
Refresh after external file changes. Restore selected
or all copies to their original graph-relative paths. **Never overwrite an
existing original**: restoration refuses conflicts and keeps the trash copy.
Restoring files does not recreate deleted notes. Only confirmed changes leave
the graph-bound preview; failures remain visible.
When versions share an original path, automatic Undo refuses to guess.
Compare their batch paths, sizes, and SHA-256 fingerprints, restore the intended
copy, then retry Undo. The explicit version choice is recorded persistently;
other copies remain in trash, and purge records prevent silent substitution.
There is no automatic purge, expiration, or disk-pressure deletion, and **no disk
space is freed** until you explicitly choose **Permanently delete** for selected
or all trash copies and confirm. **Undo cannot recover permanently deleted
attachment bytes**: Undo that needs a purged attachment fails explicitly rather
than silently restoring broken references. Tiny deletion records remain after
purge, not the attachment bytes. Keep an independent backup if needed. Trash is
device-local and never synced; removal of the original attachments will sync,
but other devices do not receive these recovery copies.

On smplOS, Auto reads `app_background_opacity` from
`current/theme/colors.toml` (decimal `0.0` through `1.0`). Older themes fall back
to `popup_opacity`, then `1.0`; invalid explicit values log an error and use opaque.
Palette edits and replacements update live. Unknown system names use the default
opaque palette, not a custom palette import. Native alpha needs a rebuilt Grafium
and one normal quit/reopen; the compositor must keep whole-window opacity at
`1.0`. Linux needs an RGBA visual and, on X11, an active compositor. On WebKitGTK
2.44 or newer, Grafium defaults to shared-memory rendering: this preserves native
background alpha without the GPU-DMABUF transport that can trigger Wayland
protocol errors. Older engines keep the safe opaque renderer. Explicit
`WEBKIT_DISABLE_DMABUF_RENDERER` or `WEBKIT_DISABLE_COMPOSITING_MODE` environment
overrides are respected; Settings explains when they prevent transparency.
Remove such an override and quit/reopen to use the default shared-memory path.

Supporting text (including flashcard hints and settings descriptions) keeps
readable contrast on solid UI surfaces; on glass it uses body-strength colors
rather than faint grey. Foreground images and text remain opaque. Since wallpaper
is outside Grafium's control, use an explicit opaque palette for predictable
reading contrast.

Auto consumes the current smplOS semantic palette, not only its name. This includes
the OLED-black Grafium theme and valid custom palettes; same-name color edits and
the Settings preview update together. Missing or invalid palettes are diagnosed
with an opaque built-in fallback. Explicit Grafium palette selections are not
overwritten by system changes.

### Useful shortcuts

| Action | Shortcut |
| --- | --- |
| Global search | `Ctrl/Cmd+K` |
| Focus the current view's filter (where available) | `Ctrl/Cmd+F` |
| Command palette | `Ctrl/Cmd+Shift+P` |
| Today's journal, ready to edit | `Ctrl/Cmd+Shift+J` |
| Go to date calendar | `Ctrl/Cmd+G` |
| Go to link (fuzzy page picker) | `Ctrl/Cmd+L` |
| Chat | `Alt+C` |
| Left / right sidebar | `Ctrl/Cmd+B` / `Ctrl/Cmd+Shift+B` |
| Bionic reading | `Ctrl/Cmd+Alt+B` |
| Private Library bookmark | `Ctrl/Cmd+Alt+M` |
| Bold selection in the block editor | `Ctrl/Cmd+Alt+Shift+B` |
| Scroll the main page, book, journal, or task pane (even while editing) | `Page Up` / `Page Down` |
| Graph / Flashcards / Tasks | `Ctrl/Cmd+Shift+G` / `F` / `T` |
| Previous / next journal | `Ctrl/Cmd+Shift+,` / `Ctrl/Cmd+Shift+.` |
| Wide / Zen mode | `Alt+W` / `Alt+Z` |
| Import media / books | `Alt+M` / `Alt+B` |
| Insert current time | `Alt+T` |
| Insert the `[[personal/diary]]` link | `Alt+D` |

Navigation mode also supports sequences such as `g j` (journal), `g g` (graph),
`g f` (flashcards), and `t w` (wide mode). They do not run as navigation commands
while you are typing in the editor.

Page Up/Down do not move the note cursor or require a click in the reading pane.
Modified keys, including Shift+Page Up/Down selection, retain their editor behavior;
menus and dialogs keep their own keyboard navigation. Graph controls are unchanged.

## Download

[Get a release build](https://github.com/KonTy/grafium/releases/latest) or
[browse all releases](https://github.com/KonTy/grafium/releases).

| Platform | Packages |
| --- | --- |
| Linux | AppImage and Debian package (`.deb`) |
| Windows | Installer (`.exe`) and MSI |
| Android | APK; check the release's signing status before installing |

Grafium is in active development. This README describes the current source;
packaged releases may not include the newest additions. Android's embedded desktop
AI/transcription engines are not included in its build. An APK explicitly named
`unsigned` needs signing before it can be installed. macOS packages are not
currently produced by the release workflow.

## Build from source

Use a stable Rust toolchain, Node.js 20 or newer, npm, and the
[Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/).
Desktop builds also compile the embedded llama.cpp and Whisper engines: install a
C/C++ toolchain, CMake, Clang/libclang, and the Vulkan development tools, including
`glslc`. A working Vulkan driver enables GPU acceleration.

On Ubuntu/Debian, the development packages include:

```bash
sudo apt install build-essential pkg-config cmake clang libclang-dev libssl-dev \
  libwebkit2gtk-4.1-dev libsoup-3.0-dev libgtk-3-dev librsvg2-dev \
  libappindicator3-dev patchelf libvulkan-dev glslc
```

On other Linux distributions, install the equivalent GTK 3, WebKitGTK 4.1,
libsoup 3, librsvg, and Vulkan development packages. Windows builds need the MSVC
C++ build tools, WebView2, and a Vulkan SDK.

Run these commands **from the repository root**:

```bash
# Install the frontend dependencies.
npm --prefix ui ci

# Run the native app in development.
npm --prefix ui run tauri -- dev

# Build the app and platform packages.
npm --prefix ui run tauri -- build

# Or build the native executable without installers.
npm --prefix ui run tauri -- build --no-bundle
```

The native executable is under `target/release/`; installers are under
`target/release/bundle/`. Desktop builds also require their generated native AI
libraries: distribute the package, not just a bare executable copied elsewhere.
Keep `custom-protocol` as a crate feature rather than enabling it directly on the
`tauri` dependency, so development loads Vite and releases embed the built frontend.

### Knowing which build you are running

The release number in `Cargo.toml` is hand-bumped, so it stays the same across
many commits and cannot tell you whether a binary is current. Every build
therefore also records the commit it was compiled from:

```bash
grafium --version
# Grafium 0.0.123 (commit f8cb86e, built 2026-09-19T05:19:07Z)
```

The same string appears in the app under **Settings → About**, so you can check
a running instance without going back to a terminal. A `dirty` marker means the
build included uncommitted changes and therefore corresponds to no commit at
all.

`scripts/deploy-local.sh` installs a local build into `~/.local` and enforces
this. It **refuses** to install when:

- the frontend in `ui/dist` is newer than the binary, which would ship the
  previous interface embedded in an otherwise-new executable, or
- the binary's recorded commit differs from the current checkout, which happens
  whenever a rebuild is skipped or fails after an earlier one succeeded.

It also warns when the working tree is dirty, and when `HEAD` is behind its
upstream branch — that last one compares against the most recently fetched
state, so run `git fetch` first for it to mean anything. Each run ends by
printing the build it installed.

These checks exist because every one of these failures is invisible: the build
succeeds, the app starts, and it simply is not the code you expected, which
reads as "my change did nothing" and sends you hunting a bug that is not there.

Local deployment now stages the executable and dereferenced native libraries in
one immutable build directory. It verifies the staged loader dependencies and
build identity, saves and verifies the previous entry points, and then switches
launchers atomically. It does not overwrite libraries mapped by a running app,
delete old builds, stop Grafium, or change graphs/settings. Restart Grafium after
deployment to use the new build.
The deployment also refreshes the Linux desktop entry and all installed icon
sizes. The window's `grafium` application ID matches its launcher, so Wayland
panels can resolve the application icon independently of the executable name.
Local launcher entries use a distinct raster icon name to avoid stale system
icons in custom menus that search system directories first. When smplOS's
`rebuild-app-cache` is available, deployment refreshes its application index too.

<details>
<summary>Optional import tools and other interfaces</summary>

For scanned PDF imports, install Poppler (`pdfinfo`, `pdftoppm`) and Tesseract.
ImageMagick's `magick` command enables additional scanned-figure extraction.
Audio/video processing needs its configured transcription model and supporting
download/conversion tools; those are separate from ordinary note editing.

The repository also contains a source-built terminal interface:

```bash
cargo run -p grafium-tui -- /path/to/your/graph
```

Pass a graph path explicitly: without one, the terminal interface uses the current
directory. It is a separate interface, not a claim of desktop feature parity.

Android builds use Tauri's Android tooling and require the JDK, Android SDK/NDK,
and target Rust toolchains. See the actual build steps in the
[release workflow](.github/workflows/release.yml).
The old standalone `android/` companion is deprecated; it is not the current app.

</details>

### Development checks

```bash
cargo test -p grafium-core
cargo test -p grafium --lib welcome
npm --prefix ui test

# Browser setup, once; then run the integration suite.
cd ui
npx playwright install chromium
npm run test:ui
```

Browser tests start their own dev server and use Tauri IPC fixtures rather than a
personal graph. They exercise interactions that unit tests alone cannot cover,
including startup, journals, selection, tables, and graph rendering.

Background-transparency coverage is included in `test:ui`. On Linux,
`python3 ui/tests/transparency.webkit.py` additionally checks native WebKitGTK
backing-store alpha using synthetic content and an isolated Xvfb display
(requires PyGObject, Cairo, GTK3 and WebKitGTK 4.1). It verifies opaque text/icon
interiors at clear, translucent and opaque background values with Grafium's
legacy renderer safeguards enabled. Snapshots alone do not prove on-screen alpha:
WebKit's legacy software renderer can return transparent snapshots while painting
an opaque native widget. After `npm --prefix ui run build`, run
`cargo test --release -p grafium --lib native_window_appearance -- --ignored --nocapture --test-threads=1`
alone on a composited Linux display, with each of `GTK_THEME=Adwaita` and
`GTK_THEME=Adwaita:dark`. The test isolates HOME/config/data, uses the compiled Tauri
configuration and real Svelte Settings, and measures the complete GTK toplevel
drawing at 0, 0.5, 0.9 and 1 with opaque glyph interiors, resizing and remapping.
It also compares native frames before and after repeated scrolling in a long
synthetic document, both while reading and editing. Explicit renderer overrides
must be unset for this default-path test; set `GRAFIUM_TEST_LEGACY_RENDERER=1`
alongside an override to check its opaque fallback and Settings explanation.
It never opens personal graphs. This is native drawing coverage, not a desktop
screenshot; the compositor must still preserve the application's alpha.

The isolated book reader has synthetic EPUB, FB2, MOBI, and PDF fixtures:
`npm --prefix ui run test:books:ui` runs them in Chromium. On Linux,
`npm --prefix ui run test:books:webkit` exercises the installed WebKitGTK 4.1
engine using Python 3/PyGObject and Xvfb, without opening a personal graph.
The reader runtime and dependency notices are generated by `predev`/`prebuild`.

Private-reader persistence and transport checks run with
`cargo test -p grafium --lib private_reader:: --no-default-features`.
The Chromium and native WebKit book suites also check bounded full-spine
narration extraction and canonical EPUB passage navigation without external
resource requests. The shared wire-contract fixture is
`ui/tests/fixtures/private-reader-position.json`.

On Linux, run
`cargo test -p grafium --lib actual_webkit_seeks_and_resumes_two_hour_external_audio --no-default-features -- --ignored`
to exercise the real private media server through WebKitGTK/GStreamer. This
requires FFmpeg, Python GI, WebKitGTK 4.1 and Xvfb; it creates a temporary silent
two-hour MP3, seeks to one hour, and resumes after a pause longer than the stream
timeout. It does not open personal media or graphs. These desktop checks do not
verify Android headphone controls or locked/screen-off physical volume keys.

### Under the hood

| Layer | Technology |
| --- | --- |
| Core | Rust: graph storage, parsing, indexing, tasks, cards, imports, and optional AI |
| Database | SQLite with WAL, FTS5, JSON1, and pooled connections |
| Interface | Svelte 5 and CodeMirror 6 |
| Native shell | Tauri 2 and the platform webview |
| Rendering | Markdown, KaTeX, 2D canvas, and Three.js-powered 3D graphs |
| Local inference | Optional llama.cpp and Whisper integrations |

Model settings/schema, managed model files, native backends, conservative resource admission, and worker supervision
live in the internal [`model-runtime`](crates/model-runtime) Rust workspace crate.
Grafium supplies graph access, credential/settings storage adapters, and UI.
The crate has no graph/database or Tauri dependency and is
not independently released. See [AI setup and runtime safety](ui/src-tauri/resources/welcome/pages/AI%20Setup%20And%20Privacy.md)
for CPU fallback and the limits of process isolation.

## Contributing and license

Bug reports, focused improvements, and documentation contributions are welcome.
[Open an issue](https://github.com/KonTy/grafium/issues) with clear reproduction
steps; use a small sample graph instead of sharing personal notes or credentials.

Grafium is licensed under the
[GNU Affero General Public License v3.0](LICENSE). Modified versions offered
over a network must make their corresponding source available to their users.
Bundled galaxy photography has its own [credits and usage information](ui/public/space/ATTRIBUTION.txt).
