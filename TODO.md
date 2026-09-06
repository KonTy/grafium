# Grafium TODO

_Exported from the Copilot CLI session todo tracker on 2026-08-04. The session's SQLite `todos` table (`~/.copilot/session-state/<session-id>/session.db`) is the live source of truth and already survives a reboot on its own; this file is a durable, human-readable backup that also captures research notes and root-cause findings that aren't stored in the DB rows themselves._

## Active

### [IN PROGRESS] Checking/enabling chat access to graph for questions (`chat-graph-access`)

Determine whether the AI chat/assistant currently has access to graph structure (pages/links) so the user can ask questions about their graph (e.g. "what links to X", "how many pages"). If not, wire it up via tool-calling or context injection.

**Research so far:** Not yet started. Need to check `core/src/ai/` chat/completion
pipeline (likely `core/src/ai/providers/*.rs` + wherever chat messages are assembled,
e.g. an `ai::chat` or `knowledge::engine` module) to see whether the graph
(pages/blocks/links) is injected into the LLM context at all today, or whether chat is
purely conversational with no tool-calling. Compare against how "Analyze"/"Ask" tabs
already pull page/backlink context (`ReferencePanel.svelte`, `core/src/ai/references.rs`)
— that pipeline may be reusable as a retrieval step for chat. Two possible designs:
(a) always inject relevant graph context via the existing embedding/reference-search
pipeline before every chat turn (RAG-style), or (b) give the model explicit tool-calling
functions (e.g. `search_pages`, `get_backlinks`, `get_page`) it can invoke on demand.
Tool-calling is more flexible but requires provider support (OpenAI/Ollama/Anthropic
function-calling formats differ) — local llama.cpp provider support for structured
tool calls needs checking too (`core/src/ai/providers/local_llm.rs`).

### [IN PROGRESS] Fixing selected-area delete button not working (`fix-selection-delete-button`)

User selected text/blocks and clicked delete button but nothing happens. Need to find the selection delete handler (likely in editor component), debug why it is a no-op, and fix it. REAL root cause found from screenshot: user was doing a native browser drag/select-all text selection across many rendered (unfocused, read-only) blocks -- NOT the bullet-click multi-select toolbar. Blocks in Grafium are only editable via CodeMirror while focused; unfocused blocks are plain read-only HTML, so browser Backspace/Delete had nothing to act on and silently did nothing (no toolbar was even shown). Fixed by adding data-block-id to each block-item DOM node and, in handleKeydownForSelection, detecting a non-collapsed window.getSelection() intersecting one or more block-item elements (outside of an active .cm-editor) on Backspace/Delete, mapping it to block ids, and running the same cascading delete used by the toolbar. Rebuilt as v0.0.112.

**Root cause (confirmed via user screenshot):** User was doing a native browser
click-drag / select-all text selection spanning multiple *unfocused* rendered blocks
— not using the bullet-click multi-select toolbar. Grafium blocks are only editable via
CodeMirror while focused; unfocused blocks render as plain read-only HTML, so the
browser's native Backspace/Delete had nothing bound to it and silently no-opped (no
toolbar even appeared).
**Fix implemented:** added `data-block-id` to every `.block-item` DOM node; in
`handleKeydownForSelection` (PageContent.svelte area), detect a non-collapsed
`window.getSelection()` that intersects one or more `.block-item` elements *outside* of
an active `.cm-editor`, map the intersected DOM nodes back to block ids via
`data-block-id`, and route Backspace/Delete through the same cascading delete already
used by the multi-select toolbar.
**Status:** implemented and rebuilt as v0.0.112. Marked in-progress because it hasn't
been re-verified against the *scrolling* variant of the original bug report (selecting
text and scrolling past the viewport bottom) — that specific scroll+select interaction
was a separate bug fixed earlier this session (virtualization jerk-back issue); worth
one more manual pass combining both: drag-select while auto-scrolling across multiple
unfocused blocks, then press Delete.

### [IN PROGRESS] Building scrollable listbox model picker with description pane (`model-picker-listbox-ui`)

Replace the "Embedded LLM Model File (GGUF)" <select> dropdown in ui/src/components/AISettings.svelte with a scrollable listbox (left) + description panel (right) showing model metadata (architecture, size, base model, etc. read via GgufContext peek without loading tensors) parsed server-side in core/src/model_library.rs and exposed via ui/src-tauri/src/commands/model_library.rs::list_local_models. Show a warning icon + explanatory text for models whose general.architecture is in the shared KNOWN_UNSTABLE_ARCHITECTURES list (qwen3next/qwen35/qwen35moe) — reuse/move that const so both local_llm.rs load-time check and this UI stay in sync.

**Design decided:** Replace the plain `<select>` "Embedded LLM Model File (GGUF)"
dropdown in `ui/src/components/AISettings.svelte` with a two-pane listbox: scrollable
model list on the left, description/metadata panel on the right (architecture, size,
base model, etc.).
**Backend groundwork:** metadata should be read via a `GgufContext` "peek" (header-only,
does NOT load tensors — important for large local model files) in
`core/src/model_library.rs`, exposed through
`ui/src-tauri/src/commands/model_library.rs::list_local_models`.
**Unstable-architecture warning:** models whose `general.architecture` matches the
shared `KNOWN_UNSTABLE_ARCHITECTURES` list (currently `qwen3next` / `qwen35` /
`qwen35moe` — architectures known to SIGSEGV in the current llama.cpp/llama-cpp-2
binding, per the local-LLM crash investigation earlier this session) should show a
warning icon + explanatory text in the description pane. That const currently lives
near the `local_llm.rs` load-time check — needs to be moved/shared (e.g. into
`core/src/model_library.rs` or a small shared const module) so both the load-time
safety check and this new UI read from the same single source of truth instead of
duplicating the architecture list.
**Related:** ties directly into the newer `model-mode-categories` request (per-category
model assignment) — worth designing the listbox data model so it can be reused/filtered
per category later instead of redoing it.

### [IN PROGRESS] Making right panel expandable and remembering width (`right-panel-expand-remember-width`)

Right side panel (backlinks/search results pane) should be resizable by dragging, and the chosen width should persist (e.g. in settings/localStorage) across restarts. Follow-up fix: the panel content wasn't showing because ReferencePanel.svelte renders its own fixed-position aside (hardcoded width:380px, its own header/tabs), completely independent of the outer wrapper div in App.svelte that had the resizer + a legacy debug "Panel is working!" placeholder stacked on top of it at a higher z-index. Removed the redundant outer wrapper, added a width prop to ReferencePanel so it uses referencePanelWidth directly, kept just the resizer element. Rebuilt as v0.0.111.

**Root cause found & fixed:** `ReferencePanel.svelte` renders its own fixed-position
`<aside>` with a hardcoded `width: 380px` plus its own header/tabs, completely
independent of the outer wrapper `<div>` in `App.svelte` that had the resizer element
and a legacy debug "Panel is working!" placeholder stacked on top of it at a higher
z-index (so the real panel content was invisibly behind the placeholder).
**Fix implemented:** removed the redundant outer wrapper div, added a `width` prop to
`ReferencePanel` so it consumes `referencePanelWidth` directly from `App.svelte` state,
kept only the resizer element for drag-to-resize. Rebuilt as v0.0.111.
**Remaining scope (why still in_progress):** the *persistence* half — saving the
user-dragged width to localStorage/settings so it survives an app restart — needs
verification. Follow the same `grafium.*` localStorage convention already used for
`grafium.pageContent.showBlockGuides` (see `App.svelte`'s
`loadShowBlockGuidesPreference()`/`setShowBlockGuides()` pattern) — add an analogous
`grafium.referencePanel.width` key with load-on-mount + save-on-drag-end.

### [PENDING] Adding Authoring tab for writing books and scientific papers (`authoring-tab-books-papers`)

New tab/mode aimed at long-form authoring (books, scientific papers) distinct from note-taking. Ties into earlier desire for models that write well without sounding like an LLM (fable-style models). Needs feature discussion: what does the authoring tab provide beyond normal page editing (outlining, chapter/section structure, citations, etc.)?

**Not yet started — design discussion needed.** New tab/mode for long-form authoring
(books, scientific papers), distinct from the note-taking-oriented page editor. Ties
into an earlier user interest in models that "write well without sounding like an LLM"
(fable-style/creative-writing-tuned models) — likely intersects with
`model-mode-categories` (an "Authoring" model category) and `llama-cpp-gdn-kda-forks`
(if a good creative-writing model needs a GDN/KDA architecture not yet supported).
Needs a feature-scope discussion before implementation: does it need chapter/section
outline structure beyond normal block nesting, citation management (for papers),
export to a manuscript format (docx/pdf/typst), word-count/target tracking, or is it
primarily "a distraction-free writing surface + a specialized model" layered on the
existing block editor.

### [PENDING] Investigating llama.cpp forks supporting GDN/KDA architectures (`llama-cpp-gdn-kda-forks`)

User found that some llama.cpp forks reportedly support GDN and KDA (hybrid attention architectures, e.g. used by newer Qwen models). Need to research these forks to see if they could unblock support for hybrid-architecture models that mainline llama.cpp cannot run yet.

**Not yet started.** User found reports that some llama.cpp forks support GDN (Gated
Delta Net) and KDA (Kimi Delta Attention / similar hybrid-linear-attention
architectures) used by newer models (e.g. Qwen3.5/Qwen3-Next family) that mainline
llama.cpp — and therefore the `llama-cpp-2`/`llama-cpp-sys-2` Rust bindings Grafium
currently vendors — cannot run without the SIGSEGV crash documented earlier this
session (`KNOWN_UNSTABLE_ARCHITECTURES` = `qwen3next`/`qwen35`/`qwen35moe`, see
checkpoints 022-025 "Fixing local LLM VRAM crash" / "Debugging persistent local-LLM
SIGSEGV crash" / "Ending silent local-LLM model fallback" / "Implementing local LLM
process isolation"). Next step: web research for named forks (e.g. search
"llama.cpp gated delta net fork", "llama.cpp KDA support" on GitHub) and assess: (a) do
they publish prebuilt bindings/crates compatible with swapping in for
`llama-cpp-sys-2`, or would it require vendoring a patched fork's C++ source directly,
(b) how actively maintained/how far behind mainline llama.cpp are they (feature/perf
tradeoff), (c) whether swapping increases build complexity (already fairly heavy —
`llama-cpp-sys-2` build script, `LD_LIBRARY_PATH` needed at runtime per the standard
relaunch procedure this session used).

### [PENDING] Splitting models/modes into categories in Grafium (`model-mode-categories`)

Grafium currently has a flat list of models/modes for AI features. User wants them split into categories, each category having its own model assignment (e.g. chat vs analysis vs authoring could each pick a different model). Needs design discussion on category taxonomy and how it interacts with existing model-picker listbox UI work.

**Not yet started — design discussion needed.** Current state: Grafium has a flat
model/mode list for AI features (one embedded GGUF model picked in AISettings, used
for chat/analyze/summarize/etc. uniformly — see `core/src/ai/providers/local_llm.rs`
and the provider selection in `core/src/knowledge/engine.rs`/`AISettings.svelte`).
User wants categories (e.g. chat vs. analysis vs. authoring vs. research) each with
their own independently-assigned model. Needs to land after/alongside
`model-picker-listbox-ui` since the new listbox UI should be designed to be reusable
per-category rather than singular. Open questions to raise with user: what are the
initial categories (chat / analyze-references / summarize / authoring / research-web
are candidates given other open todos), does each category need its own context-window
size and system prompt too, and how is the per-category choice persisted
(`core::LocalConfig` extension likely, mirroring the existing single `models_dir`/model
path settings).

### [PENDING] Adding a Research tab for live web/news question answering (`research-tab-web-news-scan`)

New tab where the user asks a question and the model goes out to scan news sites and search engines, scrapes results, and synthesizes a "what is going on" style answer. Likely reuses the existing web-scraping agent infra from the local-LLM provider work (see checkpoint 003) plus a search-engine query step; needs its own UI tab similar to Analyze/Search/Ask, with streaming results and source citations/links back to scraped pages.

**Not yet started.** New tab where the user asks a question and the model scans news
sites/search engines, scrapes results, and synthesizes a "what's going on" answer.
**Likely reusable infra:** the local-LLM provider + web-scraping agent design discussed
in checkpoint 003 ("Local LLM provider + scraping agent design") may already have
groundwork for fetching/parsing external pages — worth reviewing that checkpoint before
starting fresh. Needs: (1) a search-engine query step (no existing integration found
yet — likely needs an API key-based search provider or a scraping approach, to be
decided with the user), (2) a scrape/extract step per result, (3) a synthesis step
feeding scraped content + the user's question to the chosen model, (4) a new UI tab
alongside Analyze/Search/Ask in the right panel, ideally reusing the
`openReferencePanelTab()` / Ctrl+Shift+A/F/D hotkey pattern already established this
session for panel-tab switching, (5) streaming results with clickable source citations
back to the original scraped pages (not just an opaque summary).

### [PENDING] Fixing video/media import formatting and broken links (`video-media-import-formatting-links`)

Investigate why links are not working anymore in imported video/media-to-text content (regression reported by user). Formatting portion (title/properties/summary/transcript as nested child blocks) is now done in core/src/media/notes.rs.

**Formatting half: DONE this session.** `core/src/media/notes.rs`'s
`transcript_to_markdown()` was rewritten so the note is Title (sole top-level block) →
Source/Uploader/Duration/Transcript source/Imported/Tags bullets (visible, not hidden
`key::` properties — confirmed via exhaustive grep that Grafium's UI never renders
page- or block-level `.properties` anywhere) → `## Summary` (with per-topic sub-tree) →
`## Transcript` (with timestamped chunks) — all as real nested child blocks
(`parent_id` links) under the title. 10/10 unit tests pass, release build clean,
relaunched and confirmed running.
**Links half: NOT YET INVESTIGATED.** User reported "the links are not working anymore"
as a regression, but root cause is unknown. Starting points for next session:
grep `core/src/parser/markdown.rs` for how `[[wiki links]]` / `[markdown](links)` are
parsed into the `links` table (`core/src/db/*.rs` link-sync code, similar to
`sync_page_properties`), and check whether the new nested-bullet structure changed how
link-bearing lines are indented/parsed such that links inside transcript/summary child
blocks are no longer being picked up (e.g. a regex or indentation-depth assumption in
the link extractor that only scanned top-level blocks). Also worth checking if this
regression predates the `notes.rs` rewrite entirely (i.e. reproduce with an OLD
already-imported page, not just newly-generated ones) to isolate whether it's a parser
bug vs. a rendering bug vs. specific to media-import content.

## Completed

_Grouped by theme. Each entry's description already captures root cause, affected files, and fix approach as recorded at the time the work was done._

### Video/media import

- **Indenting imported video content under titles/headers** (`video-import-indent-under-headers`) — When importing video transcripts and generating titles/headers as blocks, content that logically belongs under a title should be indented as a child block of that title (like Logseq), not left as an adjacent sibling block. Also fixed a related pre-existing bug: generated frontmatter used --- YAML delimiters which the Grafium page parser does not recognize (only Logseq key:: value properties), causing frontmatter to render as garbage top-level blocks; switched to key:: value properties. Also switched heading content to real bullet (- ) child blocks since the parser only nests lines starting with "- ", plain indentation alone is not enough.

### Block hierarchy guide lines (bullet-threading)

- **Clarifying "show lines for blocks" request** (`block-line-numbers-clarify`) — User wants "show lines for blocks" — ambiguous, need to ask what they mean (e.g. line numbers per block, a visual line/rule separating blocks, word-wrap line indicators, etc.) before implementing.

### Hotkeys & navigation

- **Adding Ctrl-based hotkeys alongside t-r-t-h chord hotkeys** (`ctrl-hotkeys-alongside-trth`) — User likes existing t-r-t-h style hotkeys but wants Ctrl+key hotkeys added for more seamless/muscle-memory usage (avoiding sequential chords). Needs discussion on which actions get Ctrl shortcuts and how to avoid conflicts with browser/OS/Tauri default bindings.

### Local LLM

- **Isolating local LLM inference into a subprocess** (`llm-process-isolation`) — Local LLM inference (llama.cpp via llama-cpp-2/llama-cpp-sys-2, core/src/ai/providers/local_llm.rs) currently runs in-process inside the Tauri app. Native crashes (SIGSEGV, e.g. the Qwen3.5/Gated-Delta-Net ggml_compute_forward_set bug) take down the entire Grafium app. Should be moved to a separate worker process (helper binary) communicating via IPC (stdin/stdout, a local socket, or an HTTP loopback server), so a native crash in inference only kills/restarts the worker, not the whole GUI/session. This is a bigger architectural change: needs a process-lifecycle manager (spawn/monitor/restart), IPC protocol for the existing generate()/complete()/complete_stream() API surface, and graceful error propagation to the UI when the worker dies mid-request. User explicitly requested this: it should have been done regardless, and first, before further crash-symptom chasing.
- **Adding configurable shared models directory setting** (`shared-models-dir-groundwork`) — core LocalConfig.models_dir override + AISettings.svelte UI field + fixed model-path display bug + relabeled embedded-LLM UI to reflect it is not compiled into the shipped build yet.

### Bug fixes & robustness

- **Fixing assistant command case preservation** (`fix-assistant-case`) — Parse prefixes case-insensitively but preserve payload casing; add regression test, then run cargo test -p grafium-core.
- **Fixing assistant command parser mangling todo text case** (`fix-assistant-lowercase-bug`) — core/src/assistant/mod.rs lowercases the whole transcript before slicing payload, so "Add todo Fix OAuth" becomes "fix oauth". Match prefix on normalized copy but slice from original. Add regression test.
- **Fixing concept JSON parse handling** (`fix-concept-json`) — Stop silently swallowing malformed concept JSON and add regression test, then run cargo test -p grafium-core.
- **Fixing silently swallowed concept-extraction JSON parse failures** (`fix-concept-json-swallowed`) — core/src/ai/references.rs: serde_json::from_str::<Vec<ConceptJson>>(json_str).unwrap_or_default() treats malformed LLM output as "no concepts found" instead of surfacing an error. Add regression test.
- **Fixing embedding dimension validation** (`fix-dimension-validation`) — Persist and validate vector dimensions in vector store with tests, then run cargo test -p grafium-core.
- **Adding embedding dimension validation to vector store** (`fix-embedding-dimension-validation`) — core/src/knowledge/vector_store.rs only debug_asserts dimension match; release builds can silently produce wrong scores/panic with mixed-dimension vectors after switching embedding models. Persist + validate dimension. Add regression test.
- **Fixing frontend fire-and-forget save that can silently lose edits** (`fix-frontend-fireandforget-save`) — ui/src/lib/persistence.ts mutates block.content before awaiting IPC; ui/src/components/BlockEditor.svelte calls saveContent(...) without awaiting/catching on Escape/blur/stopEditing. Await + catch + rollback/surface error. Add/extend vitest regression test in persistence.test.ts.
- **Fixing swallowed graph-config parse/write errors** (`fix-graph-config-swallowed-errors`) — ui/src-tauri/src/commands/graph.rs:45-59 uses unwrap_or_default()/.ok() to swallow graph-config parse/write errors, hiding real failures from the user. Return Result and surface errors. Add regression test.
- **Fixing JournalView causing duplicate undo/redo dispatch** (`fix-journalview-double-undo`) — ui/src/components/PageContent.svelte registers global app-undo/app-redo listeners per instance; JournalView mounts many PageContent instances simultaneously, so one undo shortcut triggers multiple performUndo() calls. Route through a single root dispatcher keyed by active/focused page. Add regression test.
- **Fixing Tauri Linux setup panics on window/webview failures** (`fix-linux-window-panics`) — ui/src-tauri/src/lib.rs:1128,1135,1136,1212 use unwrap()/expect() on window/webview creation which can panic on realistic failures. Replace with graceful if-let fallbacks + logging. Add regression test if feasible (or at least a compile-time check/manual verification note).
- **Fixing inconsistent asset media hydration after renderBlock** (`fix-media-hydration-inconsistency`) — ui/src/components/Statistics.svelte and ui/src/components/PageContent.svelte (backlink previews) only import renderBlock() without calling the documented hydrateAssetMedia() follow-up, so audio/video assets in those views never hydrate. Call hydrateAssetMedia consistently wherever renderBlock output is inserted. Add regression test if feasible.
- **Fixing panic on non-UTF8 database path** (`fix-non-utf8-db-path-panic`) — core/src/graph.rs calls Database::new(db_path.to_str().unwrap()) which panics on realistic non-UTF8 Unix paths. Change Database::new to accept &Path. Add regression test if feasible.
- **Fixing stale normalized property rows on update_page** (`fix-page-properties-stale-rows`) — core/src/graph.rs / core/src/db/properties.rs: sync_page_properties() only called from index_file when parsed properties non-empty; update_page() never calls it, leaving stale rows when all properties are removed. Add regression test.
- **Fixing PageContent async load race condition** (`fix-pagecontent-async-race`) — ui/src/components/PageContent.svelte fires loadBlocks/loadBacklinks/loadHierarchy together on page change; each commits shared state after await with no request-id/version guard, so a fast page-to-page navigation can apply stale results over a newer page. Add a loadVersion guard before committing results. Add regression test.
- **Fixing literal percent characters mangled in page titles** (`fix-percent-filename-mangling`) — core/src/graph.rs title-from-filename logic does without_ext.replace("%2F", "/").replace('%', " ") which turns "100%.md" into "100 ". Only decode the legacy %2F case. Add regression test.
- **Fixing engine reconfigure rebuild** (`fix-reconfigure`) — Make KnowledgeEngine::reconfigure rebuild dependent components and add regression test, then run cargo test -p grafium-core.
- **Fixing KnowledgeEngine::reconfigure not rebuilding pipeline/reference engine** (`fix-reconfigure-stale-pipeline`) — core/src/knowledge/engine.rs reconfigure() updates self.config but never rebuilds EmbeddingPipeline/ReferenceEngine, so chunk size/thresholds dont take effect until full recreate. Add regression test.
- **Fixing ReferencePanel page_id vs page-title navigation mismatch** (`fix-referencepanel-nav-mismatch`) — ui/src/components/ReferencePanel.svelte onNavigate is typed/called with page_id, but App.svelte handleNavigate/navigateToPage expects a page title (resolves via getPage({title})). Standardize end-to-end on page id (preferred, avoids rename issues) or title consistently. Add regression test.
- **Fixing silent data loss on corrupt graph_registry.json** (`fix-registry-corrupt-json`) — core/src/knowledge/registry.rs: serde_json::from_str(&content).unwrap_or_default() silently resets registry to empty on parse failure, which then gets saved over the corrupt file. Should error/backup instead. Add regression test.
- **Fixing corrupt registry handling** (`fix-registry-corruption`) — Update registry load behavior to error on malformed JSON and add regression test, then run cargo test -p grafium-core.
- **Fixing reindex delete-before-diff data loss bug** (`fix-reindex-data-loss`) — core/src/knowledge/engine.rs + core/src/ai/embeddings.rs: reindexing deletes stored chunks before diffing what changed, and hash_cache is marked clean before upsert succeeds, causing real data loss on unchanged-page reindex or failed embeds. Fix ordering + add regression test.
- **Fixing reindex data loss** (`fix-reindex-loss`) — Inspect knowledge engine and embedding pipeline, fix diff/delete/cache ordering bug and add regression tests, then run cargo test -p grafium-core.
- **Fixing system prompt provider handling** (`fix-system-prompt`) — Ensure system_prompt is included for openai/openai_compatible/ollama and add request-body tests, then run cargo test -p grafium-core.
- **Fixing system_prompt silently dropped by 3 LLM providers** (`fix-system-prompt-providers`) — core/src/ai/providers/openai.rs, openai_compatible.rs, ollama.rs ignore CompletionOptions.system_prompt while anthropic.rs and local_llm.rs honor it. Canonicalize into shared helper. Add regression test per provider.
- **Fixing UTF-8 byte-slice panics in ai/embeddings.rs and ai/references.rs** (`fix-utf8-byte-slice-panics`) — Both slice text by raw byte offset (current[current.len()-overlap_chars..], &text[..max_len]) which panics on non-ASCII content mid-character. Use shared char-boundary-safe truncation helper. Add regression test with non-ASCII input.
- **Fixing UTF-8 safe slicing** (`fix-utf8-slices`) — Replace byte-based slicing in embeddings and references with char-boundary-safe helpers and add regression tests, then run cargo test -p grafium-core.
- **Fixing WebDavBackend::new panicking on HTTP client build failure** (`fix-webdav-backend-panic`) — core/src/sync/webdav.rs uses .build().expect("Failed to create HTTP client"). Return Result<Self> instead. Add regression test if feasible.
- **Fixing page content load races** (`ui-load-race`) — Add load-version guards to PageContent block/backlink/hierarchy loads and add a regression test for stale async results.
- **Fixing rendered media hydration** (`ui-media-hydration`) — Ensure Statistics and PageContent backlink previews hydrate rendered audio/video placeholders and add a regression test around the shared hydration hook.
- **Fixing reference navigation mismatch** (`ui-reference-navigation`) — Standardize ReferencePanel -> App navigation so page-id clicks resolve by id, while existing title-based callers keep working, and add a navigation regression test.
- **Fixing save failure handling** (`ui-save-failure`) — Update ui/src/lib/persistence.ts and ui/src/components/BlockEditor.svelte so block saves await IPC, keep unsaved edits visible on failure, and add regression coverage in ui/src/lib/persistence.test.ts.
- **Fixing duplicate undo dispatch** (`ui-undo-dispatch`) — Move global app-undo/app-redo listeners out of PageContent into a single root/shared listener and add a test proving only the intended page callback fires once.

### Performance & scalability

- **Assessing incremental on-disk patching** (`assess-incremental-patch`) — Investigate graph save and parser serializer behavior, implement a safe narrow incremental single-block patch if possible, otherwise fall back and document why.
- **Adding batched page-title lookup API to core** (`followup-core-batched-titles-api`) — core: add a function to fetch titles for a batch of page ids in one query (e.g. get_page_titles(ids: &[String]) -> HashMap<id,title>) so backlink/outlink rendering (TUI and frontend) can avoid one-lookup-per-link.
- **Attempting true incremental on-disk single-block patch** (`followup-incremental-disk-patch`) — core/src/graph.rs: single-block saves still rewrite the whole markdown file even though DB reindexing is now diffed. Attempt a safe, scoped incremental patch of just the changed block region in the on-disk file, falling back to full rewrite when the safe patch conditions are not met.
- **Wiring TUI to use new offset search + batched title APIs** (`followup-tui-wire-new-apis`) — tui/: use the new core offset-based FTS window API for load-more search paging, and the new batched title-lookup API for backlinks/outlinks instead of per-id lookups.
- **Optimizing external file indexing** (`incremental-index-diff`) — Refactor core/src/graph.rs and related db helpers so identical file rewrites short-circuit and real external edits diff by block id instead of delete-all/reinsert. Add regression tests preserving block ids and updating changed content.
- **Implementing moving PageContent window** (`pagecontent-virtual-window`) — Replace grow-only renderLimit virtualization in ui/src/components/PageContent.svelte with a real moving viewport window, plus safe block reveal support and regression tests.
- **Avoiding full page scan to compute next block order_index** (`perf-append-block-order-index`) — core/src/graph.rs around append-block logic loads all page blocks just to find max root order_index. Replace with a single SQL MAX(order_index) query.
- **Batching concept-extraction and embedding calls in ReferenceEngine** (`perf-batch-concept-embedding-calls`) — core/src/ai/references.rs: extract_concepts is called per block then embed() is called per concept - classic N+1 network round-trips. Batch concept extraction and embedding requests per page/block set.
- **Batching structural editor IPC calls (paste, indent, reorder)** (`perf-frontend-batch-structural-ops`) — ui/src/components/PageContent.svelte: multi-block paste and enter-at-start sibling shifting do sequential per-block invoke() calls. Add bulk backend ops (bulk_create_blocks, bulk_reorder) and use one IPC round-trip per structural action.
- **Debouncing sidebar search and adding backend title search** (`perf-frontend-debounce-search`) — ui/src/components/Sidebar.svelte: search runs on every keystroke with no debounce, reloading/sorting up to 10k cached pages plus a full FTS call each time. Add debounce + stale-request guards + a dedicated backend page-title search endpoint.
- **Precomputing block-tree metadata instead of repeated scans** (`perf-frontend-precompute-blocktree`) — ui/src/components/PageContent.svelte: hasChildren/isBlockVisible/getBlockDepth do repeated array scans (O(n^2)+) per rendered block. Precompute childrenByParent/parentById/depthById/visibility maps once per block-set mutation.
- **Adding true viewport virtualization to PageContent** (`perf-frontend-real-virtualization`) — ui/src/components/PageContent.svelte uses a growing renderLimit slice instead of a real moving viewport window, so long pages still accumulate unbounded DOM/editor nodes. Adopt AllPages.svelte's startIndex/endIndex windowing model.
- **Avoiding full reparse+delete-all+reinsert on external file changes** (`perf-incremental-reparse`) — core/src/graph.rs + core/src/parser/markdown.rs: external file change handling does full read+parse+delete_blocks_for_page+reinsert-everything on every watcher event. Diff old/new parsed blocks and patch only changed subtrees.
- **Making block save incremental instead of full-page rewrite** (`perf-incremental-save`) — core/src/graph.rs + core/src/parser/serializer.rs: every block save reloads all page blocks and rewrites the whole markdown file; serializer builds parent-child relationships with O(n^2) filtering. Build parent->children map once, serialize O(n), and patch only the changed region on disk where feasible.
- **Adding cheap prefilter to sync instead of full hash/download every run** (`perf-sync-hash-prefilter`) — core/src/sync/engine.rs + core/src/sync/webdav.rs + core/src/sync/filesystem.rs: FileMetadata.hash exists but is unused; sync cost scales with total bytes not actual changes. Use modified_at/size as a cheap prefilter and persist hashes locally.
- **Replacing single Mutex<Graph> with reader-friendly locking** (`perf-tauri-rwlock-graph`) — ui/src-tauri/src/lib.rs: Arc<Mutex<Graph>> serializes even read-only commands behind long-running operations like sync/reindex. Move to RwLock-style read concurrency or drop the lock before long work.
- **Streaming page indexing instead of preloading 10000 pages/blocks** (`perf-tauri-stream-reindex`) — ui/src-tauri/src/commands/knowledge.rs preloads list_pages(10000,0) plus every page block into memory before indexing, spiking memory and silently capping at 10k pages. Stream/batch page IDs instead.
- **Making page indexing transactional with prepared statements** (`perf-transactional-indexing`) — core/src/db/*.rs + core/src/graph.rs: index_file()/page rebuilds fan out into many autocommitted single-row writes across fresh pooled connections (properties, tasks, links, flashcards). Thread one rusqlite::Transaction through page indexing, prepare statements once, commit once.
- **Batching backlink/outlink title lookups and lazy-loading them** (`perf-tui-batch-backlinks`) — tui/src/panels/right_sidebar.rs + tui/src/data/repository.rs + tui/src/panels/center.rs: backlinks/outlinks do one title lookup per link (N+1) and load even when the sidebar is not visible. Add a batched core API and only load when visible.
- **Debouncing and offloading TUI search from the input thread** (`perf-tui-debounced-search`) — tui/src/panels/search_overlay.rs + tui/src/data/sources.rs + tui/src/data/repository.rs: search reruns synchronous FTS on every keystroke on the single UI thread with no debounce, and paginates via skip() over a refetched window instead of a real offset query. Add debounce, move DB work off the input-handling thread, add offset-based FTS window API in core.
- **Making TUI redraw event-driven with cached rendered lines** (`perf-tui-event-driven-redraw`) — tui/src/main.rs polls/redraws unconditionally every 200ms and reparses markdown every frame in center.rs/markdown_view.rs. Redraw only on input/resize/tick-needed, and cache rendered+wrapped lines per block keyed by content hash + width.
- **Moving vector search off the async runtime with bounded top-k** (`perf-vector-search-async`) — core/src/knowledge/vector_store.rs: search() runs synchronous rusqlite + full deserialize + full sort on the async runtime. Use spawn_blocking and a bounded top-k heap instead of sorting all matches.
- **Debouncing Sidebar search** (`sidebar-search-guard`) — Add debounced Sidebar search execution, stale-request guards, and isolated vitest coverage for the debounce/race behavior without changing results.
- **Batching knowledge indexing page loads** (`tauri-index-batching`) — Stream or batch page IDs/blocks for knowledge indexing instead of preloading 10k pages; add batch-processing test.
- **Implementing batched backlink titles** (`tui-backlinks-batch`) — Replace per-page title lookups in get_backlinks with one get_page_titles call and update tests to assert a single batched lookup.
- **Debouncing and offloading TUI search** (`tui-search-debounce`) — Inspect search overlay and data source/repository; add debounced background search and improve windowing without core changes; add debounce/coalescing tests.

### Search / title-lookup APIs

- **Adding FTS window search API** (`add-fts-window-api`) — Implement a paged FTS block search API in core/src/db/blocks.rs, keep existing search_fts working, and add regression tests.
- **Adding page title lookup and search APIs** (`add-page-title-apis`) — Implement batched page title lookup and indexed title search in core/src/db/pages.rs and schema support if needed, with regression tests.
- **Adding offset-based FTS search window API to core** (`followup-core-search-window-api`) — core: add a real LIMIT/OFFSET-based full-text-search window API (e.g. search_fts_window(query, limit, offset)) so callers like the TUI can page through results without re-querying with a growing limit and client-side skip.
- **Adding indexed page-title search API to core** (`followup-core-title-search-api`) — core: add a dedicated indexed page-title search function/command so the frontend Sidebar can search titles without loading+client-side-scanning up to 10k cached pages per keystroke.
- **Wiring Sidebar to use backend indexed title search** (`followup-frontend-title-search`) — ui/src-tauri + ui/src: expose the new core title-search API as a Tauri command and use it from Sidebar.svelte instead of client-side scanning cached pages.
- **Implementing frontend indexed title search** (`frontend-title-search`) — Add Tauri command for search_page_titles and update sidebarSearch.ts/tests to invoke it with debounce and stale-result guards preserved.

### TUI (terminal UI)

- **Fixing TUI losing unsaved edits on failed save** (`fix-tui-save-before-insert-exit`) — tui/src/widgets/editor_pane.rs exits insert mode before persisting; tui/src/panels/center.rs converts save failure into a status string and open_page() ignores the result entirely; CenterPanel::error is set but never rendered in draw(). Keep editor active until save succeeds, block navigation on failure, render the error. Add regression test.
- **Deferring and deduplicating backlink and outlink loads** (`tui-links-lazyload`) — Load backlinks/outlinks only when sidebar or graph is visible and reduce N+1 title lookups via deduplication/client-side batching; add visibility test.
- **Optimizing TUI redraw and markdown rendering** (`tui-redraw-cache`) — Inspect tui main loop and markdown rendering; add dirty-driven redraws and cache rendered markdown lines by content hash and width; add focused cache/dirty tests.
- **Fixing TUI save/navigation handling** (`tui-save-guard`) — Update editor and center panel save flow so failed saves keep edits, block navigation, render errors, and add regression tests under tui/src.
- **Implementing TUI search paging** (`tui-search-paging`) — Update GraphRepository search_blocks to take offset, wire CoreRepository to search_fts_window, update search_overlay paging logic and related mocks/tests.

### Sync

- **Optimizing sync metadata comparisons** (`sync-hash-prefilter`) — Use FileMetadata size/modified_at plus cached sync-state hashes in core/src/sync to avoid whole-file hashing on unchanged runs, and add mock-backed tests.

### Validation / investigation

- **Inspecting existing DB and parser code** (`inspect-existing-db-and-parser`) — Read current block/page DB APIs, schema, graph save flow, and parser serializer support to design the new core APIs and assess safe incremental patching.
- **Inspecting relevant files** (`inspect-files`) — Read the Rust and frontend files involved in repository search, backlinks, sidebar search, and test conventions.
- **Validating workspace and frontend** (`validate`) — Run cargo build/test for workspace and npm test/build in ui, then summarize pass/fail and changed files.
- **Validating core tests and workspace build** (`validate-core-and-workspace`) — Run cargo test -p grafium-core --lib --tests after each change and finish with cargo build --workspace.
- **Running workspace validation** (`workspace-validate`) — Run targeted tests plus cargo build/test for the workspace (excluding feature-gated examples) and report final results.
- **Validating workspace build and tests** (`workspace-validation`) — Ran cargo build --workspace successfully after final changes. Full workspace cargo test now fails in unrelated core test sync_test::test_multiple_syncs_with_incremental_changes; ui/src-tauri and tui targeted tests pass.

### Other fixes & improvements

- **Propagating graph config errors** (`graph-config-errors`) — Return Result for graph config load/save in ui/src-tauri/src/commands/graph.rs, update callers, and add regression tests for malformed config and save failures.
- **Optimizing PageContent block derivations** (`pagecontent-derived-maps`) — Precompute PageContent children/parent/depth/visibility maps once per block-set mutation and add parity tests proving identical results to the legacy logic.
- **Optimizing root append order lookup** (`root-order-query`) — Replace full page block scan in Graph::next_order_index_for_page with a SQL MAX(order_index) helper and add a targeted test.
- **Optimizing page serialization** (`serializer-optimization`) — Refactor core/src/parser/serializer.rs to build a parent->children map once, preserve byte-identical output, and add behavior/performance tests. Scope item 1 without risky incremental disk patching.
- **Hardening Linux webview setup** (`tauri-linux-guards`) — Replace panic-prone unwrap/expect calls in ui/src-tauri/src/lib.rs Linux window/webview setup with graceful warnings; add practical regression coverage or document limits.
- **Reducing Tauri graph lock contention** (`tauri-lock-scope`) — Inspect ui/src-tauri state and commands; narrow lock scope for long sync/reindex ops and evaluate RwLock safety; add focused test if practical.
- **Making page indexing transactional** (`transactional-indexing`) — Thread a single rusqlite transaction through page indexing and derived-state writes in core/src/db and core/src/graph.rs, plus atomicity tests including rollback on simulated failure.

