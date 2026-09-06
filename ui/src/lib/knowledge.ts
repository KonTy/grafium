// Knowledge Engine API — AI, references, vector search, schemas.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

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
  done: boolean;
  error?: string | null;
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

export function aiIndexPage(pageId: string): Promise<number> {
  return invoke("ai_index_page", { pageId });
}

export function aiIndexAllPages(): Promise<number> {
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

export function aiAsk(question: string, graphId?: string): Promise<string> {
  return invoke("ai_ask", { question, graphId });
}

export async function aiAskStream(
  question: string,
  handlers: {
    onChunk: (delta: string) => void;
    onDone: () => void;
    onError?: (message: string) => void;
  },
  graphId?: string
): Promise<void> {
  const requestId = `${Date.now()}-${Math.random().toString(36).slice(2)}`;

  const unlisten = await listen<StreamChunk>("ai://chat_stream", (event) => {
    const payload = event.payload;
    if (!payload || payload.request_id !== requestId) return;

    if (payload.error) {
      handlers.onError?.(payload.error);
      return;
    }

    if (payload.delta) {
      handlers.onChunk(payload.delta);
    }

    if (payload.done) {
      handlers.onDone();
    }
  });

  try {
    await invoke("ai_ask_stream", { question, graphId, requestId });
  } catch (e: any) {
    handlers.onError?.(String(e));
  } finally {
    unlisten();
  }
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
