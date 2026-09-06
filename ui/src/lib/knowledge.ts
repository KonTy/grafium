// Knowledge Engine API — AI, references, vector search, schemas.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ─── Types ───────────────────────────────────────────────────────────────────

export interface AiConfig {
  enabled: boolean;
  mode: "local" | "cloud" | "hybrid";
  local?: {
    provider: string;
    base_url: string;
    api_key?: string;
    models_dir?: string;
    local_llm?: {
      model?: string;
      context_size?: number;
      gpu_layers?: number;
      use_mmap?: boolean | null;
    };
    local_embedding?: {
      model?: string;
    };
    llm_model: string;
    embedding_model: string;
  };
  cloud?: {
    llm_provider: string;
    llm_model: string;
    llm_api_key?: string;
    llm_base_url?: string;
    embedding_provider: string;
    embedding_model: string;
    embedding_api_key?: string;
    embedding_base_url?: string;
  };
}

export interface AiConfigPayload {
  enabled: boolean;
  mode: string;
  local_provider?: string;
  local_base_url?: string;
  local_api_key?: string;
  local_model_path?: string;
  local_context_size?: number;
  local_gpu_layers?: number;
  /// null = auto (use built-in heuristic); true/false = force mmap on/off.
  local_use_mmap?: boolean | null;
  local_embedding_model_path?: string;
  local_models_dir?: string;
  llm_model?: string;
  embedding_model?: string;
  cloud_provider?: string;
  cloud_base_url?: string;
  cloud_llm_model?: string;
  cloud_api_key?: string;
  cloud_embedding_provider?: string;
  cloud_embedding_base_url?: string;
  cloud_embedding_api_key?: string;
  cloud_embedding_model?: string;
}

export interface StreamChunk {
  request_id: string;
  delta: string;
  /** Current answering phase on a transition, else null; never carries reasoning text. */
  phase?: string | null;
  /** Transient per-phase progress note (e.g. "Reading source 2/5: …") during a
   *  web-research pass. Display-only; never part of the answer text. */
  note?: string | null;
  done: boolean;
  error?: string | null;
}

// A structured citation returned alongside a Chat answer, so the UI can render
// clickable source chips and navigate to the originating page/block.
export interface ChatSource {
  index: number;
  page_id: string;
  page_title: string;
  block_id: string;
  date?: string | null;
}

// A web source cited by the "From the web" section of a research answer. Unlike
// a ChatSource (which points at a local page/block), this is an external URL the
// UI renders as a clickable link opened in the system browser. `number` matches
// the inline `[n]` marker in the streamed web summary.
export interface WebSource {
  number: number;
  title: string;
  url: string;
}

export interface SourcesPayload {
  request_id: string;
  sources: ChatSource[];
  /** Web sources for a research answer; empty/absent for an ordinary answer. */
  web_sources?: WebSource[];
}

export interface AskResult {
  answer: string;
  sources: ChatSource[];
}

// Compact label for a source chip, e.g. "[3] 2026-03-14 · Journal" or
// "[1] Rust". Pure so it can be unit-tested and reused.
export function formatSourceLabel(source: ChatSource): string {
  const parts = [`[${source.index}]`];
  if (source.date) parts.push(source.date);
  parts.push(source.page_title);
  return parts.join(" · ");
}

// Compact label for a web-source chip, e.g. "[2] example.com · How creatine
// works". Falls back to the raw URL when it can't be parsed. Pure and reusable.
export function formatWebSourceLabel(source: WebSource): string {
  let host = "";
  try {
    host = new URL(source.url).hostname.replace(/^www\./, "");
  } catch {
    host = source.url;
  }
  const title = source.title.trim();
  const label = title && title !== host ? `${host} · ${title}` : host;
  return `[${source.number}] ${label}`;
}

// Whether the Chat empty-index banner should be shown: the status has loaded
// and there are zero indexed chunks. A null status (not yet loaded / errored)
// keeps the banner hidden.
export function shouldShowIndexBanner(
  indexedChunks: number | null
): boolean {
  return indexedChunks === 0;
}

export interface HealthStatus {
  enabled: boolean;
  llm_available: boolean;
  embedder_available: boolean;
  vector_store_available: boolean;
  vector_count: number;
  mode: string;
  llm_load_error?: string | null;
}

export interface SemanticSearchResult {
  chunk_id: string;
  graph_id: string;
  page_id: string;
  block_id: string | null;
  page_title: string;
  content: string;
  score: number;
  metadata: Record<string, unknown>;
}

export interface GeneratedReference {
  ref_number: number;
  block_id: string;
  anchor_text: string;
  anchor_offset: number;
  reference_text: string;
  related_pages: RelatedPage[];
  confidence: number;
  generated_at: number;
}

export interface RelatedPage {
  page_id: string;
  page_title: string;
  graph_id: string;
  score: number;
  snippet: string;
}

export interface TagTerm {
  term: string;
  qualified?: string | null;
}

export interface TopicSummary {
  topic: string;
  summary: string;
  tags: TagTerm[];
}

export interface PageSummary {
  title_answer: string | null;
  topics: TopicSummary[];
}

export interface PageReferencesMeta {
  page_id: string;
  generated_at: number;
  content_hash: string;
  reference_count: number;
  references: GeneratedReference[];
  summary: PageSummary | null;
  // When `summary` is null because generation was *attempted and failed*
  // (as opposed to skipped because the page had no eligible blocks),
  // this holds the human-readable reason from the backend so the UI can
  // render an actionable "why there's no summary" notice instead of
  // silently showing nothing.
  summary_error?: string | null;
}

// A cited web source found and read by "Web Research".
export interface Citation {
  number: number;
  title: string;
  url: string;
}

// A topic's cited summary paragraph — same shape as TopicSummary, but
// `summary` contains inline "[n]" markers pointing into the parent
// WebResearchResult's `citations` list, since claims here come from
// external sources rather than content already in the page.
export interface ResearchTopic {
  topic: string;
  summary: string;
  tags: TagTerm[];
}

export interface WebResearchResult {
  title_answer: string | null;
  topics: ResearchTopic[];
  citations: Citation[];
}

export interface RegisteredGraph {
  id: string;
  name: string;
  path: string;
  graph_type: string;
  last_indexed: number | null;
  page_count: number | null;
  vector_count: number | null;
  cross_searchable: boolean;
  description: string | null;
}

export interface Schema {
  tag: string;
  display_name: string;
  icon: string | null;
  description: string | null;
  fields: SchemaField[];
  template: string | null;
  ai_auto_classify: boolean;
}

export interface SchemaField {
  key: string;
  label: string;
  field_type: string;
  required: boolean;
  default: unknown;
  options: string[] | null;
  ai_autofill: boolean;
  description: string | null;
}

// ─── Configuration ───────────────────────────────────────────────────────────

export function aiGetConfig(): Promise<AiConfig> {
  return invoke("ai_get_config");
}

export function aiSetConfig(payload: AiConfigPayload): Promise<void> {
  return invoke("ai_set_config", { payload });
}

// ─── Health ──────────────────────────────────────────────────────────────────

export function aiHealthCheck(): Promise<HealthStatus> {
  return invoke("ai_health_check");
}

// ─── Indexing ────────────────────────────────────────────────────────────────

/** GPU/CPU status of the local chat model, mirrors core's `AcceleratorStatus`. */
export interface AcceleratorStatus {
  /** Whether this build can offload to a GPU at all. When false, CPU is expected. */
  gpu_supported: boolean;
  /** Whether inference is actually running on the GPU. */
  on_gpu: boolean;
  /** Effective GPU layers; 0 means CPU-only. */
  gpu_layers: number;
  /** Free VRAM observed at load time (MiB), or null if unqueryable. */
  free_vram_mib_at_load: number | null;
  /** Model file size (MiB). */
  model_mib: number | null;
  /** Whether gpu_layers was pinned explicitly in config. */
  explicit: boolean;
}

export interface IndexStatus {
  indexed_chunks: number;
  total_blocks: number;
  /** Pages edited since their last index, awaiting a background reindex. */
  pending_pages: number;
  embedder_ready: boolean;
  llm_ready: boolean;
  /** Local LLM GPU/CPU status, or null for remote providers / no LLM loaded. */
  accelerator: AcceleratorStatus | null;
}

export function aiIndexStatus(): Promise<IndexStatus> {
  return invoke("ai_index_status");
}

/**
 * Reload the local chat model forcing full GPU offload — Chat's "Retry on
 * GPU" action. Returns the refreshed accelerator status (or null).
 */
export function aiRetryLlmOnGpu(): Promise<AcceleratorStatus | null> {
  return invoke("ai_retry_llm_on_gpu");
}

export function aiIndexPage(pageId: string): Promise<number> {
  return invoke("ai_index_page", { pageId });
}

export interface IndexAllResult {
  indexed_chunks: number;
  pages_processed: number;
  pages_failed: number;
}

export function aiIndexAllPages(): Promise<IndexAllResult> {
  return invoke("ai_index_all_pages");
}

// ─── Search ──────────────────────────────────────────────────────────────────

export function aiSearch(
  query: string,
  topK?: number,
  graphId?: string
): Promise<SemanticSearchResult[]> {
  return invoke("ai_search", { query, topK, graphId });
}

// ─── References ──────────────────────────────────────────────────────────────

export function aiGenerateReferences(
  pageId: string,
  operationId?: string
): Promise<PageReferencesMeta> {
  return invoke("ai_generate_references", { pageId, operationId });
}

// Summarizes an arbitrary text selection (e.g. concatenated content of
// selected blocks), returning the same title-answer/per-topic-summary
// shape as `PageReferencesMeta.summary` (one paragraph + tags per distinct
// topic covered) — used by "Analyze Selected" in PageContent.svelte to
// insert a summary block right after a selection.
export function aiSummarizeSelection(
  text: string,
  title?: string,
  operationId?: string
): Promise<PageSummary> {
  return invoke("ai_summarize_selection", { text, title, operationId });
}

// Actually researches `title`/`seedText` on the open internet — plans
// search queries, searches, reads the most relevant results, and returns a
// cited topic-by-topic summary (inline "[n]" markers pointing at real
// source URLs in the returned `citations`). Unlike `aiGenerateReferences`/
// `aiSummarizeSelection`, this can take a while (multiple web fetches) and
// reports progress via the "ai-web-research-progress" event.
export function aiResearchWeb(
  title: string,
  seedText: string,
  operationId?: string
): Promise<WebResearchResult> {
  return invoke("ai_research_web", { title, seedText, operationId });
}

// Signal the backend that the user pressed Cancel on the progress toast
// for a currently-running AI operation. Flips a cancellation token that
// the engine cooperatively checks between steps, and also hard-kills the
// local LLM worker (llama.cpp's C++ inference loop checks nothing between
// tokens, so we can't stop it any other way). Safe to call for an
// unknown/finished operationId — it's a no-op in that case.
export function aiCancelOperation(operationId: string): Promise<void> {
  return invoke("ai_cancel_operation", { operationId });
}

// Wraps the first verbatim, whole-word occurrence of each term found in
// `content` with `[[wiki-link]]` syntax (optionally substituting a
// `qualified` disambiguation phrase in place of the matched text). Thin
// wrapper around the shared core `wrap_known_terms_as_links` function —
// used by both "Analyze Selected" and "Insert into page" so term-wrapping
// logic lives in one place.
export function wrapKnownTermsInText(content: string, terms: TagTerm[]): Promise<string> {
  return invoke("text_wrap_known_terms", { content, terms });
}

// Result of a successful `aiInsertPageSummary` call — the fields needed to
// push a matching entry onto the undo stack (see undoStack.ts) so Ctrl-Z
// after "Insert into page" cleanly reverses the full change, including
// the tag-wrap rewrites of unrelated pre-existing blocks.
export interface SummaryWrapChange {
  blockId: string;
  previousContent: string;
  newContent: string;
}

export interface AiInsertSummaryResult {
  insertedBlockId: string;
  insertedContent: string;
  insertedAfterBlockId: string | null;
  wrapChanges: SummaryWrapChange[];
}

// Inserts an AI-generated page summary as a new block on the page — one
// heading + paragraph per topic — and wraps each topic's tags in place as
// `[[wiki-link]]`s across the page's existing blocks. Used by the "Insert
// into page" button in ReferencePanel.svelte.
//
// `afterBlockId` is the id of the block the user last had a caret in on
// the current page — when provided, the summary is inserted immediately
// after that block (so it lands "at the cursor" wherever they were
// reading); when null, it falls back to the top of the page.
//
// The returned `AiInsertSummaryResult` carries exactly what undo needs
// to reverse the write — the id of the freshly-created summary block
// plus the previous/new content of every block whose text was rewritten
// during tag-wrap.
export function aiInsertPageSummary(
  pageId: string,
  titleAnswer: string | null,
  topics: TopicSummary[],
  afterBlockId: string | null = null
): Promise<AiInsertSummaryResult> {
  return invoke("ai_insert_page_summary", { pageId, titleAnswer, topics, afterBlockId });
}

// Undoes a previous aiInsertPageSummary — deletes the summary block and
// restores each rewrapped block to its previous content. Best-effort per
// block: skips any that have since been deleted rather than aborting.
export function aiUndoSummaryInsert(
  insertedBlockId: string,
  wrapChanges: SummaryWrapChange[]
): Promise<void> {
  return invoke("ai_undo_summary_insert", { insertedBlockId, wrapChanges });
}

// Redoes a previously-undone summary insert — recreates the summary
// block after the same anchor (falling back to top-of-page if the anchor
// has since been deleted) and reapplies each wrap change. Returns a
// fresh AiInsertSummaryResult so the undo stack can flip the redo entry
// back into an undo entry with the new block id.
export function aiReapplySummaryInsert(
  pageId: string,
  insertedContent: string,
  insertedAfterBlockId: string | null,
  wrapChanges: SummaryWrapChange[]
): Promise<AiInsertSummaryResult> {
  return invoke("ai_reapply_summary_insert", {
    pageId,
    insertedContent,
    insertedAfterBlockId,
    wrapChanges,
  });
}

// ─── RAG / Ask ───────────────────────────────────────────────────────────────

export function aiAsk(question: string, graphId?: string): Promise<AskResult> {
  return invoke("ai_ask", { question, graphId });
}

/** One prior message in the Chat transcript, as the backend expects it. */
export interface ChatTurn {
  role: "user" | "assistant";
  content: string;
}

export async function aiAskStream(
  question: string,
  handlers: {
    onChunk: (delta: string) => void;
    onDone: () => void;
    onError?: (message: string) => void;
    onSources?: (sources: ChatSource[]) => void;
    onWebSources?: (sources: WebSource[]) => void;
    onPhase?: (phase: string) => void;
    onNote?: (note: string) => void;
    onStart?: (requestId: string) => void;
  },
  graphId?: string,
  /** Prior turns, oldest first. Sent whole — the backend decides how much to
   *  replay verbatim and compacts the rest. */
  history?: ChatTurn[]
): Promise<void> {
  const requestId = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  handlers.onStart?.(requestId);

  // Acquire both listeners inside the guarded block with nullable handles.
  // Previously the two listen() calls ran before the try, so a rejection from
  // the first left Chat stuck "active" with onError never called, and a
  // rejection from the second leaked the first subscription. Now any setup
  // failure routes through onError and whatever was acquired is always removed.
  let unlistenStream: UnlistenFn | null = null;
  let unlistenSources: UnlistenFn | null = null;
  try {
    unlistenStream = await listen<StreamChunk>("ai://chat_stream", (event) => {
      const payload = event.payload;
      if (!payload || payload.request_id !== requestId) return;

      if (payload.error) {
        handlers.onError?.(payload.error);
        return;
      }

      // A phase transition (retrieving / processing_prompt / thinking /
      // generating / searching_web / reading_sources) carries no answer text — it
      // drives the status indicator.
      if (payload.phase) {
        handlers.onPhase?.(payload.phase);
      }

      // A progress note (e.g. "Reading source 2/5: …") is display-only detail for
      // the current phase; it is never appended to the answer.
      if (payload.note) {
        handlers.onNote?.(payload.note);
      }

      if (payload.delta) {
        handlers.onChunk(payload.delta);
      }

      if (payload.done) {
        handlers.onDone();
      }
    });

    unlistenSources = await listen<SourcesPayload>("ai://chat_sources", (event) => {
      const payload = event.payload;
      if (!payload || payload.request_id !== requestId) return;
      handlers.onSources?.(payload.sources ?? []);
      if (payload.web_sources && payload.web_sources.length > 0) {
        handlers.onWebSources?.(payload.web_sources);
      }
    });

    await invoke("ai_ask_stream", { question, graphId, requestId, history: history ?? [] });
  } catch (e: any) {
    // A user Stop cancels the run; when that surfaces as the canonical
    // cancellation rejection it's a normal end, not an error to report.
    if (!isResearchCancellation(String(e))) handlers.onError?.(String(e));
  } finally {
    unlistenStream?.();
    unlistenSources?.();
  }
}

// Cancel an in-flight streamed answer. The local generation loop checks the
// flag and stops, returning what it has so far; a no-op if already finished.
export function aiCancelStream(requestId: string): Promise<void> {
  return invoke("ai_cancel_stream", { requestId });
}

// The backend rejects a cancelled web-research run with this exact message
// (core/src/ai/web_research.rs RESEARCH_CANCELLED). Stopping a run is a normal,
// expected end — not a failure to surface — so both stream wrappers use this to
// tell a genuine error apart from the user pressing Stop.
export const RESEARCH_CANCELLED_MESSAGE = "Web research was cancelled.";

export function isResearchCancellation(message: string): boolean {
  return message.includes(RESEARCH_CANCELLED_MESSAGE);
}

// ─── Graph Registry ──────────────────────────────────────────────────────────

export function aiListRegisteredGraphs(): Promise<RegisteredGraph[]> {
  return invoke("ai_list_registered_graphs");
}

export function aiRegisterGraph(
  name: string,
  path: string,
  graphType: string
): Promise<void> {
  return invoke("ai_register_graph", { name, path, graphType });
}

// ─── Schemas ─────────────────────────────────────────────────────────────────

export function aiListSchemas(): Promise<Schema[]> {
  return invoke("ai_list_schemas");
}

export function aiSaveSchema(schema: Schema): Promise<void> {
  return invoke("ai_save_schema", { schema });
}

export function aiCreateDefaultSchemas(): Promise<void> {
  return invoke("ai_create_default_schemas");
}
