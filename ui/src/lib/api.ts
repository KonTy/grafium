import { invoke } from "@tauri-apps/api/core";

export interface Page {
  id: string;
  title: string;
  /** Graph-relative path of the page's markdown file, e.g.
   *  `pages/mybooks/coolbook/toc.md`. `null` until the page has content,
   *  since a page referenced by a link exists in the index before any file
   *  is written for it. */
  file_path?: string | null;
  is_journal: boolean;
  created_at: string;
  updated_at: string;
  properties: Record<string, unknown>;
}

export interface PageSummary {
  id: string;
  title: string;
  is_journal: boolean;
}

export interface Block {
  id: string;
  page_id: string;
  parent_id: string | null;
  order_index: number;
  content: string;
  block_type: string;
  properties: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export interface SearchResult {
  id: string;
  page_id: string;
  content: string;
}

export interface BacklinkResult {
  link: unknown;
  block: Block;
}

// Pages
export function listPages(limit = 100, offset = 0): Promise<Page[]> {
  return invoke("list_pages", { limit, offset });
}

export function countPages(): Promise<number> {
  return invoke("count_pages");
}

export function listPagesWindow(limit: number, offset: number, sortByTitle: boolean): Promise<Page[]> {
  return invoke("list_pages_window", { limit, offset, sortByTitle });
}

export function listJournalPages(limit = 20, offset = 0): Promise<Page[]> {
  return invoke("list_journal_pages", { limit, offset });
}

export function getPage(opts: { id?: string; title?: string }): Promise<Page> {
  return invoke("get_page", opts);
}

export function createPage(title: string, isJournal = false): Promise<Page> {
  return invoke("create_page", { title, isJournal });
}

export function updatePageMeta(id: string, title?: string, properties?: Record<string, unknown>): Promise<void> {
  return invoke("update_page_meta", { id, title, properties });
}

export function deletePage(id: string): Promise<void> {
  return invoke("delete_page", { id });
}

export function getParentPage(title: string): Promise<Page | null> {
  return invoke("get_parent_page", { title });
}

export function getChildPages(parentTitle: string): Promise<Page[]> {
  return invoke("get_child_pages", { parentTitle });
}

export function searchPageTitles(query: string, limit = 10): Promise<PageSummary[]> {
  return invoke("search_page_titles", { query, limit });
}

// Blocks
export function listBlocks(pageId: string): Promise<Block[]> {
  return invoke("list_blocks", { pageId });
}

export function createBlock(
  pageId: string,
  parentId: string | null,
  orderIndex: number,
  content: string,
  blockType?: string,
  properties?: Record<string, unknown>
): Promise<Block> {
  return invoke("create_block", { pageId, parentId, orderIndex, content, blockType, properties });
}

export function updateBlock(id: string, content: string, properties?: Record<string, unknown>): Promise<void> {
  return invoke("update_block", { id, content, properties });
}

export function deleteBlock(id: string): Promise<void> {
  return invoke("delete_block", { id });
}

export function moveBlock(id: string, newParentId: string | null, orderIndex: number): Promise<void> {
  return invoke("move_block", { id, newParentId, orderIndex });
}

export function reorderBlocks(pageId: string, blockIds: string[]): Promise<void> {
  return invoke("reorder_blocks", { pageId, blockIds });
}

export function searchFts(query: string, limit = 50): Promise<Block[]> {
  return invoke("search_fts", { query, limit });
}

// Links
export function getBacklinks(pageId: string): Promise<BacklinkResult[]> {
  return invoke("get_backlinks", { pageId });
}

// Tasks
export function listTasks(taskState?: string): Promise<unknown[]> {
  return invoke("list_tasks", { taskState });
}

export function updateTaskState(blockId: string, newState: string): Promise<void> {
  return invoke("update_task_state", { blockId, newState });
}

export function cycleTaskState(blockId: string): Promise<string> {
  return invoke("cycle_task_state", { blockId });
}

export type { OpenTaskRow, TaskFlowStats } from "./taskBoard";

/** Every open task with its dates, for grouping by when it is due. */
export function listOpenTaskRows(): Promise<import("./taskBoard").OpenTaskRow[]> {
  return invoke("list_open_task_rows", {});
}

/** Flow metrics for the Tasks dashboard. */
export function taskFlowStats(weeks = 12): Promise<import("./taskBoard").TaskFlowStats> {
  return invoke("task_flow_stats", { weeks });
}

export interface BackfillReport {
  pages_scanned: number;
  tasks_updated: number;
  backup_path: string | null;
  dry_run: boolean;
}

/** Write completion times held only in the database into the markdown.
 *  Always call with `dryRun` first — a real run edits notes in bulk. */
export function backfillTaskCompletions(dryRun: boolean): Promise<BackfillReport> {
  return invoke("backfill_task_completions", { dryRun });
}

export function setTaskDate(blockId: string, kind: "scheduled" | "deadline", date: string | null): Promise<string> {
  return invoke("set_task_date", { blockId, kind, date });
}

export interface CompletedTask {
  timestamp: number;
  content: string;
  page_title: string;
  block_id: string;
}

export function getCompletionCounts(days?: number): Promise<[string, number][]> {
  return invoke("get_completion_counts", { days });
}

export function getCompletedTasks(days?: number): Promise<CompletedTask[]> {
  return invoke("get_completed_tasks", { days });
}

export interface OpenTask {
  timestamp: number;
  content: string;
  page_title: string;
  block_id: string;
  state: string;
}

export function getOpenTasks(days?: number): Promise<OpenTask[]> {
  return invoke("get_open_tasks", { days });
}

/** Response from the shared voice-assistant NLU (`grafium_core::assistant`).
 *  Backs both the desktop UI and the Android AssistantReceiver JNI shim. */
export interface AssistantResponse {
  speech: string;
  followup: boolean;
}

/** Send a raw voice / text transcript to the shared Rust NLU. The same code
 *  path runs on Android via JNI, so the two platforms cannot drift. */
export function handleAssistantCommand(transcript: string): Promise<AssistantResponse> {
  return invoke("handle_assistant_command", { transcript });
}

export function getBlockPageTitle(blockId: string): Promise<string> {
  return invoke("get_block_page_title", { blockId });
}

// Flashcards
export interface Flashcard {
  id: string;
  block_id: string;
  front: string;
  back: string;
  tags: string[];
  created_at: number;
  updated_at: number;
  last_reviewed_at: number | null;
  next_review_at: number | null;
  ease_factor: number;
  interval_days: number;
  review_count: number;
  /** Graph-relative path of the page the card came from, when it has a file.
   *  Review happens away from the page, so media stored beside that page needs
   *  its directory to resolve. */
  page_file_path?: string | null;
}

export interface FlashcardTopic {
  topic: string; // "" = untagged
  total: number;
  due: number;
}

// topic: a tag name (e.g. "chinese"); "" = untagged; omit for mixed (all topics).
export function listFlashcardsDue(limit?: number, topic?: string): Promise<Flashcard[]> {
  return invoke("list_flashcards_due", { limit, topic });
}

export function listFlashcardTopics(): Promise<FlashcardTopic[]> {
  return invoke("list_flashcard_topics", {});
}

export function listAllFlashcards(limit?: number, offset?: number): Promise<Flashcard[]> {
  return invoke("list_all_flashcards", { limit, offset });
}

// quality: 0..5 (0-2 = fail, 3-5 = pass). Returns the updated card.
export function gradeFlashcard(id: string, quality: number): Promise<Flashcard> {
  return invoke("grade_flashcard", { id, quality });
}

export interface AnkiImportSummary {
  deck: string;
  page_title: string;
  topic: string;
  note_count: number;
  card_count: number;
  media_count: number;
}

// Import an Anki .apkg deck into the active graph. Converts it to a markdown
// page of `Front :: Back` flashcards tagged with the deck's topic and copies
// referenced media into the graph assets folder.
export function importAnkiApkg(path: string): Promise<AnkiImportSummary> {
  return invoke("import_anki_apkg", { path });
}

// Favorites & Recent
export function addFavorite(pageId: string): Promise<void> {
  return invoke("add_favorite", { pageId });
}

export function removeFavorite(pageId: string): Promise<void> {
  return invoke("remove_favorite", { pageId });
}

export function listFavorites(): Promise<Page[]> {
  return invoke("list_favorites", {});
}

export function recordPageOpen(pageId: string): Promise<void> {
  return invoke("record_page_open", { pageId });
}

export function listRecentPages(limit = 10): Promise<Page[]> {
  return invoke("list_recent_pages", { limit });
}

// Query — returns rows as arrays of [column_name, value] pairs
export type QueryRow = [string, unknown][];
export function runQuery(query: string): Promise<QueryRow[]> {
  return invoke("run_query", { queryString: query });
}

// Graph Management
export interface GraphInfo {
  name: string;
  path: string;
}

export interface GraphValidationReport {
  is_valid: boolean;
  has_pages_dir: boolean;
  has_journals_dir: boolean;
  has_metadata_dir: boolean;
  has_valid_db: boolean;
  not_nested_in_another_graph: boolean;
  has_no_nested_graph_roots: boolean;
  error_message: string | null;
}

export function getGraphInfo(): Promise<GraphInfo> {
  return invoke("get_graph_info", {});
}

export interface GraphNode {
  id: string;
  title: string;
  degree: number;
}

export interface GraphEdge {
  source: string;
  target: string;
  weight: number;
}

export interface GraphData {
  nodes: GraphNode[];
  edges: GraphEdge[];
}

export function getGraphData(nodeLimit: number, focusPageId?: string): Promise<GraphData> {
  return invoke("get_graph_data", { nodeLimit, focusPageId: focusPageId ?? null });
}

export function listGraphs(): Promise<GraphInfo[]> {
  return invoke("list_graphs", {});
}

export function validateGraph(path: string): Promise<GraphValidationReport> {
  return invoke("validate_graph", { path });
}

export function openGraph(path: string): Promise<GraphInfo> {
  return invoke("open_graph", { path });
}

export function createGraph(path: string, name: string): Promise<GraphInfo> {
  return invoke("create_graph", { path, name });
}

export function reindexCurrent(): Promise<void> {
  return invoke("reindex_current", {});
}

export function removeGraph(path: string): Promise<void> {
  return invoke("remove_graph", { path });
}

export function getAppVersion(): Promise<string> {
  return invoke("get_app_version", {});
}

export interface DirEntry {
  name: string;
  path: string;
  is_dir: boolean;
}

export interface DirListing {
  current_path: string;
  entries: DirEntry[];
}

export function listDirectory(path: string): Promise<DirListing> {
  return invoke("list_directory", { path });
}

export function getDefaultGraphBase(): Promise<string> {
  return invoke("get_default_graph_base", {});
}

// Theme
export function getSmplosTheme(): Promise<string | null> {
  return invoke("get_smplos_theme", {});
}

export function getAppTheme(): Promise<string> {
  return invoke("get_app_theme", {});
}

export function setAppTheme(themeId: string): Promise<void> {
  return invoke("set_app_theme", { themeId });
}

// Asset management
export function downloadAsset(url: string, pageId?: string): Promise<string> {
  return invoke("download_asset", { url, pageId });
}

export function listAssets(): Promise<string[]> {
  return invoke("list_assets", {});
}

export interface OrphanedAsset {
  /** Graph-relative path, e.g. `assets/x.png` or
   *  `pages/mybooks/coolbook/assets/x.png`. Media can sit beside its own page,
   *  so a bare name would not say which file is meant. */
  filename: string;
  size: number;
}

export function findOrphanedAssets(): Promise<OrphanedAsset[]> {
  return invoke("find_orphaned_assets", {});
}

/** Delete media by graph-relative path, as reported by `findOrphanedAssets`. */
export function deleteAssets(filenames: string[]): Promise<number> {
  return invoke("delete_assets", { filenames });
}

// Media import (video/audio transcript -> page, or appended to today's journal)
export type MediaImportTarget = "new_page" | "journal";

export function mediaImportVideo(
  url: string,
  pageTitle?: string,
  lang?: string,
  target?: MediaImportTarget,
): Promise<Page> {
  return invoke("media_import_video", { url, pageTitle, lang, target });
}

// Media / Whisper transcription settings
export interface MediaConfig {
  enabled: boolean;
  models_dir?: string;
  whisper: {
    model?: string;
    language?: string;
  };
}

export interface MediaConfigPayload {
  enabled: boolean;
  models_dir?: string;
  whisper_model_path?: string;
  language?: string;
}

export function mediaGetConfig(): Promise<MediaConfig> {
  return invoke("media_get_config");
}

export function mediaSetConfig(payload: MediaConfigPayload): Promise<void> {
  return invoke("media_set_config", { payload });
}

// Local model library (browse locally-downloaded model files for dropdowns)
export interface LocalModelInfo {
  file_name: string;
  size_bytes: number;
  kind: "llm" | "whisper" | "embedding" | "reranker" | "unknown";
  /** How this model is expected to perform on the current GPU. Only
   *  meaningful for `kind: "llm"`; other kinds always report `"unknown"`. */
  gpu_fit: "fits" | "tight" | "cpu_only" | "unknown";
  /** Plain-English rationale for `gpu_fit`, safe to render verbatim. */
  gpu_fit_detail: string;
}

export function listLocalModels(modelsDir?: string): Promise<LocalModelInfo[]> {
  return invoke("list_local_models", { modelsDir });
}
