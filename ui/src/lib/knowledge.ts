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

// A structured citation returned alongside a Chat answer, so the UI can render
// clickable source chips and navigate to the originating page/block.
export interface ChatSource {
  index: number;
  page_id: string;
  page_title: string;
  block_id: string;
  date?: string | null;
}

export interface SourcesPayload {
  request_id: string;
  sources: ChatSource[];
}

export interface AskResult {
  answer: string;
  sources: ChatSource[];
}

export interface HealthStatus {
  enabled: boolean;
  llm_available: boolean;
  embedder_available: boolean;
  vector_store_available: boolean;
  vector_count: number;
  mode: string;
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

export interface IndexStatus {
  indexed_chunks: number;
  total_blocks: number;
  embedder_ready: boolean;
  llm_ready: boolean;
}

export function aiIndexStatus(): Promise<IndexStatus> {
  return invoke("ai_index_status");
}

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

export function aiGenerateReferences(pageId: string): Promise<PageReferencesMeta> {
  return invoke("ai_generate_references", { pageId });
}

// Summarizes an arbitrary text selection (e.g. concatenated content of
// selected blocks), returning the same title-answer/per-topic-summary
// shape as `PageReferencesMeta.summary` (one paragraph + tags per distinct
// topic covered) — used by "Analyze Selected" in PageContent.svelte to
// insert a summary block right after a selection.
export function aiSummarizeSelection(text: string, title?: string): Promise<PageSummary> {
  return invoke("ai_summarize_selection", { text, title });
}

// Actually researches `title`/`seedText` on the open internet — plans
// search queries, searches, reads the most relevant results, and returns a
// cited topic-by-topic summary (inline "[n]" markers pointing at real
// source URLs in the returned `citations`). Unlike `aiGenerateReferences`/
// `aiSummarizeSelection`, this can take a while (multiple web fetches) and
// reports progress via the "ai-web-research-progress" event.
export function aiResearchWeb(title: string, seedText: string): Promise<WebResearchResult> {
  return invoke("ai_research_web", { title, seedText });
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

// Inserts an AI-generated page summary as a new block at the top of the
// page (right after the title) — one heading + paragraph per topic — and
// wraps each topic's tags in place as `[[wiki-link]]`s across the page's
// existing blocks. Used by the "Insert into page" button in
// ReferencePanel.svelte.
export function aiInsertPageSummary(
  pageId: string,
  titleAnswer: string | null,
  topics: TopicSummary[]
): Promise<void> {
  return invoke("ai_insert_page_summary", { pageId, titleAnswer, topics });
}

// ─── RAG / Ask ───────────────────────────────────────────────────────────────

export function aiAsk(question: string, graphId?: string): Promise<AskResult> {
  return invoke("ai_ask", { question, graphId });
}

export async function aiAskStream(
  question: string,
  handlers: {
    onChunk: (delta: string) => void;
    onDone: () => void;
    onError?: (message: string) => void;
    onSources?: (sources: ChatSource[]) => void;
  },
  graphId?: string
): Promise<void> {
  const requestId = `${Date.now()}-${Math.random().toString(36).slice(2)}`;

  const unlistenStream = await listen<StreamChunk>("ai://chat_stream", (event) => {
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

  const unlistenSources = await listen<SourcesPayload>("ai://chat_sources", (event) => {
    const payload = event.payload;
    if (!payload || payload.request_id !== requestId) return;
    handlers.onSources?.(payload.sources ?? []);
  });

  try {
    await invoke("ai_ask_stream", { question, graphId, requestId });
  } catch (e: any) {
    handlers.onError?.(String(e));
  } finally {
    unlistenStream();
    unlistenSources();
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
