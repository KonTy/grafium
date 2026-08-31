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

export interface PageReferencesMeta {
  page_id: string;
  generated_at: number;
  content_hash: string;
  reference_count: number;
  references: GeneratedReference[];
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

/**
 * Start a full reindex. Resolves with a job id as soon as the work is
 * *queued*, not when it finishes — track progress via the jobs store.
 */
export function aiIndexAllPages(): Promise<string> {
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

/**
 * Start reference generation for a page. Resolves with a job id; the finished
 * references arrive via {@link onReferencesGenerated}, so navigating away no
 * longer discards the work.
 */
export function aiGenerateReferences(pageId: string): Promise<string> {
  return invoke("ai_generate_references", { pageId });
}

/**
 * Subscribe to finished reference payloads.
 *
 * References are computed rather than stored, so they arrive on this channel
 * instead of as a command return value. The handler receives results for every
 * page, so callers must filter to the one they care about.
 */
export function onReferencesGenerated(
  handler: (pageId: string, meta: PageReferencesMeta) => void
): Promise<UnlistenFn> {
  return listen<{ page_id: string; meta: PageReferencesMeta }>(
    "ai://references",
    (event) => handler(event.payload.page_id, event.payload.meta)
  );
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
