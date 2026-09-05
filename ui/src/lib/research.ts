// Deep Research API — search-engine registry, agentic-workflow prompts, and the
// multi-round `research_deep` stream. Mirrors the shape and register of
// `knowledge.ts` on purpose: same `invoke`/`listen` plumbing, same streaming
// handler callbacks, so the Chat "Research" path is a drop-in for `aiAskStream`.
//
// Contract: RESEARCH_CONTRACT.md (frozen). The core side is built in parallel,
// so every wrapper here degrades gracefully when its command isn't registered
// yet — callers get a thrown error to catch, and Settings falls back to
// `DEFAULT_RESEARCH_CONFIG` so the page still renders.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { StreamChunk, SourcesPayload, ChatSource, WebSource } from "./knowledge";
import { isResearchCancellation } from "./knowledge";

// ─── Payload casing (the single flip point) ──────────────────────────────────
//
// The contract does not pin whether the Rust research structs carry
// `#[serde(rename_all = "camelCase")]`. We assume they DON'T: every existing
// payload in `knowledge.ts` (`request_id`, `page_id`, `url_template`-style
// fields) is snake_case, and the contract's own TypeScript examples spell the
// fields `url_template`, `json_paths`, `max_rounds`, … So the canonical shape
// below is snake_case and equals the wire shape as-is.
//
// If core turns camelCase on, flip THIS ONE constant to "camel". The
// `toWire`/`fromWire` boundary then re-maps every field name in both directions
// and nothing else — no interface edits, no call-site churn. Command *argument*
// names (`requestId`, `graphId`) are never affected: Tauri already converts
// those to snake_case Rust params itself, exactly as `ai_ask_stream` relies on.
export type PayloadCase = "snake" | "camel";
export const PAYLOAD_CASE: PayloadCase = "snake";

function snakeToCamelKey(key: string): string {
  return key.replace(/_([a-z0-9])/g, (_m, c: string) => c.toUpperCase());
}

function camelToSnakeKey(key: string): string {
  return key.replace(/[A-Z]/g, (c) => "_" + c.toLowerCase());
}

// Recursively rewrite object *keys* (never values) so enum payloads like
// `kind: "Html"` and templated URLs like `"…?q={query}"` pass through untouched.
function mapKeysDeep(input: unknown, keyFn: (k: string) => string): unknown {
  if (Array.isArray(input)) return input.map((v) => mapKeysDeep(v, keyFn));
  if (input !== null && typeof input === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(input as Record<string, unknown>)) {
      out[keyFn(k)] = mapKeysDeep(v, keyFn);
    }
    return out;
  }
  return input;
}

/** Canonical (snake_case) → wire, driven entirely by {@link PAYLOAD_CASE}. */
export function snakeToCamelDeep<T = unknown>(value: unknown): T {
  return mapKeysDeep(value, snakeToCamelKey) as T;
}

/** Wire → canonical (snake_case), driven entirely by {@link PAYLOAD_CASE}. */
export function camelToSnakeDeep<T = unknown>(value: unknown): T {
  return mapKeysDeep(value, camelToSnakeKey) as T;
}

function toWire<T>(value: T): unknown {
  return PAYLOAD_CASE === "camel" ? snakeToCamelDeep(value) : value;
}

function fromWire<T>(value: unknown): T {
  return PAYLOAD_CASE === "camel" ? camelToSnakeDeep<T>(value) : (value as T);
}

// ─── Types (mirror core/src/research/config.rs) ──────────────────────────────

export type EngineKind = "Html" | "Json";
export type EngineCategory = "Web" | "Academic";

/** CSS selectors for scraping an `EngineKind::Html` result page. */
export interface HtmlSelectors {
  result: string;
  link: string;
  title: string;
  snippet: string;
}

/** Dotted JSON paths, relative to the response root, for an `EngineKind::Json` API. */
export interface JsonPaths {
  results: string;
  url: string;
  title: string;
  snippet: string;
  /**
   * Absolute prefix prepended to each item's `url` when the API returns a
   * relative identifier rather than a link (Google Patents → "patent/US…",
   * Open Library → "/works/OL123W"). Without it those citations are relative,
   * and `openWebSource()` rejects anything that isn't http(s). Null/omitted for
   * APIs that already return absolute URLs, which keeps existing engine
   * definitions serializing unchanged.
   */
  url_prefix?: string | null;
}

export interface SearchEngineDef {
  id: string;
  name: string;
  kind: EngineKind;
  /** Query URL with a literal `{query}` placeholder, URL-encoded on substitution. */
  url_template: string;
  enabled: boolean;
  /** Built-ins can be disabled but never deleted. */
  builtin: boolean;
  category: EngineCategory;
  /** Present only for `kind: "Html"`. */
  selectors?: HtmlSelectors | null;
  /** Present only for `kind: "Json"`. */
  json_paths?: JsonPaths | null;
}

/** One editable prompt per agentic step. Field order == pipeline order. */
export interface ResearchPrompts {
  plan_queries: string;
  select_sources: string;
  assess_sufficiency: string;
  refine_queries: string;
  synthesize: string;
}

export interface ResearchConfig {
  engines: SearchEngineDef[];
  prompts: ResearchPrompts;
  max_rounds: number;
  max_sources: number;
  results_per_query: number;
  ocr_enabled: boolean;
}

// Result shape for the Test button. The contract names `SearchResultPayload`
// but does not spell its fields, so we mirror the extraction targets shared by
// `HtmlSelectors`/`JsonPaths` (title/url/snippet) — see the report note. The
// Test UI relies only on `title` (with `url` as fallback), so a divergent field
// on the core side degrades to an empty string rather than breaking.
export interface SearchResult {
  title: string;
  url: string;
  snippet: string;
}

// ─── Config commands ─────────────────────────────────────────────────────────

export async function researchGetConfig(): Promise<ResearchConfig> {
  const raw = await invoke("research_get_config");
  return fromWire<ResearchConfig>(raw);
}

export function researchSetConfig(config: ResearchConfig): Promise<void> {
  return invoke("research_set_config", { payload: toWire(config) });
}

/** Restores the shipped defaults for the *prompts only*, returning the full config. */
export async function researchResetPrompts(): Promise<ResearchConfig> {
  const raw = await invoke("research_reset_prompts");
  return fromWire<ResearchConfig>(raw);
}

/**
 * True when an IPC rejection means the command simply isn't registered in this
 * build (Tauri answers "Command <name> not found", or a capability-denied one
 * with "not allowed"), as opposed to the command running and failing — e.g. a
 * corrupt/unreadable `research_config.json` surfacing a serde/IO message. The
 * distinction matters in Settings: a missing command means degrade-to-defaults
 * with Save disabled, but a real load error must keep Save *enabled* so the
 * user can overwrite the bad file instead of hitting a dead end.
 */
export function isCommandNotRegistered(message: string): boolean {
  const m = message.toLowerCase();
  return (
    (m.includes("command") && m.includes("not found")) ||
    m.includes("not allowed") ||
    m.includes("not registered") ||
    m.includes("unknown command")
  );
}

/**
 * Runs a single engine against `query` so a student can tell "my selector is
 * wrong" (0 results / parse error) from "that engine is blocking me" (network
 * error). Rejects with the backend's error string on failure.
 */
export async function researchTestEngine(
  engine: SearchEngineDef,
  query: string,
): Promise<SearchResult[]> {
  const raw = await invoke("research_test_engine", { engine: toWire(engine), query });
  return fromWire<SearchResult[]>(raw);
}

// ─── Deep research stream ────────────────────────────────────────────────────

// Same callback surface as `aiAskStream` so ChatView can swap one for the other
// without touching its rendering. `research_deep` streams over the very same
// `ai://chat_stream` / `ai://chat_sources` channels an ordinary answer uses.
export interface ResearchStreamHandlers {
  onChunk: (delta: string) => void;
  onDone: () => void;
  onError?: (message: string) => void;
  onSources?: (sources: ChatSource[]) => void;
  onWebSources?: (sources: WebSource[]) => void;
  onPhase?: (phase: string) => void;
  onNote?: (note: string) => void;
  onStart?: (requestId: string) => void;
}

/**
 * Forces the full multi-round deep-research workflow, bypassing the intent
 * classifier entirely. `requestId` is generated when omitted (mirroring
 * `aiAskStream`) and handed back via `onStart` so the caller can cancel with
 * {@link researchCancel}.
 */
export async function researchDeep(
  question: string,
  handlers: ResearchStreamHandlers,
  requestId: string = `${Date.now()}-${Math.random().toString(36).slice(2)}`,
  graphId?: string,
): Promise<void> {
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
      // planning / searching_web / reading_sources / assessing / refining /
      // synthesizing — drives the status indicator, never answer text.
      if (payload.phase) handlers.onPhase?.(payload.phase);
      // "Reading source 2 of 6: <title>" — display-only progress detail.
      if (payload.note) handlers.onNote?.(payload.note);
      if (payload.delta) handlers.onChunk(payload.delta);
      if (payload.done) handlers.onDone();
    });

    unlistenSources = await listen<SourcesPayload>("ai://chat_sources", (event) => {
      const payload = event.payload;
      if (!payload || payload.request_id !== requestId) return;
      handlers.onSources?.(payload.sources ?? []);
      if (payload.web_sources && payload.web_sources.length > 0) {
        handlers.onWebSources?.(payload.web_sources);
      }
    });

    await invoke("research_deep", { question, requestId, graphId });
  } catch (e: any) {
    // A user Stop rejects the invoke with the canonical cancellation message;
    // that's a normal end to the run, not a failure to surface.
    if (!isResearchCancellation(String(e))) handlers.onError?.(String(e));
  } finally {
    unlistenStream?.();
    unlistenSources?.();
  }
}

/** Cancels an in-flight deep-research run; a no-op if it already finished. */
export function researchCancel(requestId: string): Promise<void> {
  return invoke("research_cancel", { requestId });
}

// ─── Workflow step metadata ──────────────────────────────────────────────────

export interface PromptStepMeta {
  key: keyof ResearchPrompts;
  /** Short, human label for the step. */
  label: string;
  /** One plain-English sentence: what this prompt controls and when it runs.
   *  Audience is a student, not the person who wrote the pipeline. */
  explanation: string;
}

// Presented in this order so the Settings page reads top-to-bottom as the
// actual pipeline: plan → select → assess → refine (loop) → synthesize.
export const RESEARCH_PROMPT_STEPS: PromptStepMeta[] = [
  {
    key: "plan_queries",
    label: "Plan searches",
    explanation:
      "Runs first: turns your question into a handful of specific web searches to try.",
  },
  {
    key: "select_sources",
    label: "Choose what to read",
    explanation:
      "After each search: decides which of the results are worth actually opening and reading in full.",
  },
  {
    key: "assess_sufficiency",
    label: "Check if it's enough",
    explanation:
      "After reading: judges whether what's been gathered can answer your question, or whether another round is needed.",
  },
  {
    key: "refine_queries",
    label: "Refine the search",
    explanation:
      "When more is needed: writes sharper follow-up searches to fill the gaps, then the search loop repeats.",
  },
  {
    key: "synthesize",
    label: "Write the summary",
    explanation:
      "Runs last: writes the final, cited answer from everything that was read.",
  },
];

// ─── Defaults (UI fallback + Reset) ──────────────────────────────────────────

// The core's `ResearchConfig::default()` is authoritative, and each step's
// model output is parsed there with `serde_json::from_str` into a specific
// shape (`{"queries":[…]}`, `{"picks":[…]}`, the assessment/synthesis objects —
// see `core/src/research/config.rs`). A prompt that asks for plain lines/prose
// would make the backend fail to parse and silently break a research run, so a
// per-field "Reset to default" must restore the *exact* backend default, not an
// approximation. These therefore mirror the Rust `default_*` prompts
// byte-for-byte; `researchPromptsDrift.test.ts` reads that file and fails the
// build if the two ever diverge. Keep them offline (no IPC) so the reset works
// even when the backend command isn't registered yet.
export const DEFAULT_RESEARCH_PROMPTS: ResearchPrompts = {
  plan_queries:
    `You are a research assistant planning how to investigate a question on the web. Given the user's question, produce up to 4 focused, diverse search-engine queries that together would surface authoritative sources answering it. Prefer specific, well-formed queries over broad ones; where the topic is scholarly, include an academic phrasing. Do not answer the question yourself. Reply with ONLY a JSON object of the form {"queries": ["...", "..."]} — no other text, no markdown fences.`,
  select_sources:
    `You are choosing which search results are worth reading in full to answer a research question. You are given the question and a numbered list of candidate results (title, URL, snippet). Pick the most relevant, credible, and diverse results — avoid near-duplicates and low-quality sources. Reply with ONLY a JSON object of the form {"picks": [<indices>]} listing the indices to read, most useful first — no other text, no markdown fences.`,
  assess_sufficiency:
    `You are judging whether the material gathered so far is enough to write a well-supported, cited answer to a research question. You are given the question and excerpts from the sources read so far. Be honest: if key claims are unsupported, sources conflict without resolution, or an important angle is missing, it is NOT sufficient yet. Reply with ONLY a JSON object of the form {"sufficient": true|false, "missing": "one sentence naming what is still needed (empty if sufficient)"} — no other text, no markdown fences.`,
  refine_queries:
    `You are improving a web-research run that does not yet have enough to answer the question. You are given the question, the titles already gathered, and a note on what is still missing. Produce up to 4 NEW search-engine queries targeting the gap — do not repeat the earlier queries, and get more specific or approach from a different angle. Reply with ONLY a JSON object of the form {"queries": ["...", "..."]} — no other text, no markdown fences.`,
  synthesize:
    `You are a careful research assistant writing the final answer from real, numbered web sources. Use ONLY the numbered sources provided — do not add outside knowledge and do not invent facts. Identify every distinct topic worth reporting. Return a JSON object with:\n- "title_answer": if the question can be answered directly, one sentence answering it with an inline [n] citation; otherwise null.\n- "topics": an array of objects, each with "topic" (a short label), "summary" (a 2-5 sentence paragraph where EVERY factual claim ends with an inline citation like "[1]" or "[2][4]"; if sources disagree, say so), and "tags" (1-4 key-term objects {"term": "..."}).\nReturn ONLY the JSON object, no other text, no markdown fences.`,
};

function html(id: string, name: string, url_template: string, selectors: HtmlSelectors): SearchEngineDef {
  return { id, name, kind: "Html", url_template, enabled: true, builtin: true, category: "Web", selectors };
}

function academicJson(id: string, name: string, url_template: string, json_paths: JsonPaths): SearchEngineDef {
  return { id, name, kind: "Json", url_template, enabled: true, builtin: true, category: "Academic", json_paths };
}

// Built-in engines, per the contract. No shadow libraries (LibGen / Anna's
// Archive / Sci-Hub) — the schema is user-extensible, so that stays the user's
// own decision, never a shipped default.
export const DEFAULT_ENGINES: SearchEngineDef[] = [
  html("brave", "Brave", "https://search.brave.com/search?q={query}", {
    result: "#results .snippet",
    link: "a",
    title: ".title",
    snippet: ".snippet-description",
  }),
  html("duckduckgo", "DuckDuckGo", "https://html.duckduckgo.com/html/?q={query}", {
    result: ".result",
    link: ".result__a",
    title: ".result__a",
    snippet: ".result__snippet",
  }),
  html("mojeek", "Mojeek", "https://www.mojeek.com/search?q={query}", {
    result: "ul.results-standard li",
    link: "a.title",
    title: "a.title",
    snippet: "p.s",
  }),
  html("startpage", "Startpage", "https://www.startpage.com/sp/search?query={query}", {
    result: ".w-gl__result",
    link: ".w-gl__result-title",
    title: ".w-gl__result-title",
    snippet: ".w-gl__description",
  }),
  academicJson("openalex", "OpenAlex", "https://api.openalex.org/works?search={query}&per-page=10", {
    results: "results",
    url: "id",
    title: "display_name",
    snippet: "abstract",
  }),
  academicJson("crossref", "Crossref", "https://api.crossref.org/works?query={query}&rows=10", {
    results: "message.items",
    url: "URL",
    title: "title.0",
    snippet: "abstract",
  }),
  // arXiv is Atom XML, not JSON. Per the contract we model it as `Html` with
  // XML-ish element selectors for the fallback; core may swap in a dedicated
  // parser instead — either way the backend's config wins once it loads.
  {
    id: "arxiv",
    name: "arXiv",
    kind: "Html",
    url_template: "http://export.arxiv.org/api/query?search_query=all:{query}&max_results=10",
    enabled: true,
    builtin: true,
    category: "Academic",
    selectors: { result: "entry", link: "id", title: "title", snippet: "summary" },
  },
  academicJson(
    "europepmc",
    "Europe PMC",
    "https://www.ebi.ac.uk/europepmc/webservices/rest/search?query={query}&format=json&pageSize=10",
    { results: "resultList.result", url: "doi", title: "title", snippet: "abstractText" },
  ),
  academicJson(
    "semanticscholar",
    "Semantic Scholar",
    "https://api.semanticscholar.org/graph/v1/paper/search?query={query}&fields=title,abstract,url,year&limit=10",
    { results: "data", url: "url", title: "title", snippet: "abstract" },
  ),
  academicJson("doaj", "DOAJ", "https://doaj.org/api/search/articles/{query}?pageSize=10", {
    results: "results",
    url: "bibjson.link.0.url",
    title: "bibjson.title",
    snippet: "bibjson.abstract",
  }),
];

export function defaultResearchConfig(): ResearchConfig {
  return {
    // Deep-clone so a caller mutating the working copy can't corrupt the module
    // constant (the Settings form binds and edits this shape directly).
    engines: DEFAULT_ENGINES.map((e) => ({
      ...e,
      selectors: e.selectors ? { ...e.selectors } : e.selectors,
      json_paths: e.json_paths ? { ...e.json_paths } : e.json_paths,
    })),
    prompts: { ...DEFAULT_RESEARCH_PROMPTS },
    max_rounds: 3,
    max_sources: 8,
    results_per_query: 6,
    ocr_enabled: false,
  };
}

// Sane bounds for the numeric knobs — kept beside the defaults so the Settings
// inputs and any validation share one source of truth.
export const RESEARCH_LIMITS = {
  max_rounds: { min: 1, max: 10 },
  max_sources: { min: 1, max: 50 },
  results_per_query: { min: 1, max: 20 },
} as const;

export function clampResearchNumbers(config: ResearchConfig): ResearchConfig {
  const clamp = (v: number, lo: number, hi: number) =>
    Math.max(lo, Math.min(hi, Math.round(Number.isFinite(v) ? v : lo)));
  return {
    ...config,
    max_rounds: clamp(config.max_rounds, RESEARCH_LIMITS.max_rounds.min, RESEARCH_LIMITS.max_rounds.max),
    max_sources: clamp(config.max_sources, RESEARCH_LIMITS.max_sources.min, RESEARCH_LIMITS.max_sources.max),
    results_per_query: clamp(
      config.results_per_query,
      RESEARCH_LIMITS.results_per_query.min,
      RESEARCH_LIMITS.results_per_query.max,
    ),
  };
}

// ─── Add-engine form validation (pure, unit-tested) ──────────────────────────

// A draft mirrors SearchEngineDef but every field is optional/loose, since it's
// bound straight to the add-engine form inputs before it's known to be valid.
export interface EngineDraft {
  id?: string;
  name?: string;
  kind?: EngineKind;
  url_template?: string;
  category?: EngineCategory;
  selectors?: Partial<HtmlSelectors>;
  json_paths?: Partial<JsonPaths>;
}

// A stable slug: lowercase letters/digits with single hyphens between, so it can
// key config and round-trip through the backend cleanly.
const SLUG_RE = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

/**
 * Returns the list of human-readable problems with a draft engine, empty when
 * it's good to add. `existingIds` blocks collisions with engines already in the
 * config (built-in or user). Only the selector/path fields for the *chosen*
 * kind are required, matching the conditional form.
 */
export function validateEngineDraft(draft: EngineDraft, existingIds: string[] = []): string[] {
  const errors: string[] = [];
  const id = (draft.id ?? "").trim();
  const name = (draft.name ?? "").trim();
  const url = (draft.url_template ?? "").trim();

  if (!id) {
    errors.push("An ID is required.");
  } else if (!SLUG_RE.test(id)) {
    errors.push("ID must be a slug: lowercase letters, digits, and single hyphens (e.g. \"my-engine\").");
  } else if (existingIds.includes(id)) {
    errors.push(`ID "${id}" is already in use.`);
  }

  if (!name) errors.push("A display name is required.");

  if (!url) {
    errors.push("A URL template is required.");
  } else if (!url.includes("{query}")) {
    errors.push("The URL template must include a {query} placeholder.");
  }

  if (draft.kind === "Html") {
    const s = draft.selectors ?? {};
    if (!(s.result ?? "").trim()) errors.push("HTML engines need a result (container) selector.");
    if (!(s.link ?? "").trim()) errors.push("HTML engines need a link selector.");
    if (!(s.title ?? "").trim()) errors.push("HTML engines need a title selector.");
  } else if (draft.kind === "Json") {
    const p = draft.json_paths ?? {};
    if (!(p.results ?? "").trim()) errors.push("JSON engines need a results (array) path.");
    if (!(p.url ?? "").trim()) errors.push("JSON engines need a url path.");
    if (!(p.title ?? "").trim()) errors.push("JSON engines need a title path.");
    // A prefix only earns its keep by turning a relative identifier into a
    // citable link, so a non-http value would still fail openWebSource().
    const prefix = (p.url_prefix ?? "").trim();
    if (prefix && !/^https?:\/\//i.test(prefix)) {
      errors.push("The URL prefix must be an absolute http(s):// URL (it's prepended to relative result URLs).");
    }
  } else {
    errors.push("Choose an engine kind (HTML or JSON).");
  }

  return errors;
}

// Builds a clean SearchEngineDef from a validated draft — drops the selector or
// path block that doesn't apply to the chosen kind so the payload stays tidy.
export function engineFromDraft(draft: EngineDraft): SearchEngineDef {
  const base: SearchEngineDef = {
    id: (draft.id ?? "").trim(),
    name: (draft.name ?? "").trim(),
    kind: draft.kind === "Json" ? "Json" : "Html",
    url_template: (draft.url_template ?? "").trim(),
    enabled: true,
    builtin: false,
    category: draft.category === "Academic" ? "Academic" : "Web",
    selectors: null,
    json_paths: null,
  };
  if (base.kind === "Html") {
    const s = draft.selectors ?? {};
    base.selectors = {
      result: (s.result ?? "").trim(),
      link: (s.link ?? "").trim(),
      title: (s.title ?? "").trim(),
      snippet: (s.snippet ?? "").trim(),
    };
  } else {
    const p = draft.json_paths ?? {};
    base.json_paths = {
      results: (p.results ?? "").trim(),
      url: (p.url ?? "").trim(),
      title: (p.title ?? "").trim(),
      snippet: (p.snippet ?? "").trim(),
      // Empty → null so the backend's Option<String> stays None and engines
      // that need no prefix serialize exactly as before.
      url_prefix: (p.url_prefix ?? "").trim() || null,
    };
  }
  return base;
}
