import { invoke } from "@tauri-apps/api/core";

export interface Page {
  id: string;
  title: string;
  is_journal: boolean;
  created_at: string;
  updated_at: string;
  properties: Record<string, unknown>;
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

// Pages
export function listPages(limit = 100, offset = 0): Promise<Page[]> {
  return invoke("list_pages", { limit, offset });
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
export function getBacklinks(pageId: string): Promise<{ link: unknown; block: Block }[]> {
  return invoke("get_backlinks", { pageId });
}

// Tasks
export function listTasks(taskState?: string): Promise<unknown[]> {
  return invoke("list_tasks", { taskState });
}

export function updateTaskState(blockId: string, newState: string): Promise<void> {
  return invoke("update_task_state", { blockId, newState });
}

// Flashcards
export function listFlashcardsDue(): Promise<unknown[]> {
  return invoke("list_flashcards_due", {});
}

export function listAllFlashcards(): Promise<unknown[]> {
  return invoke("list_all_flashcards", {});
}

export function updateFlashcardReview(blockId: string, quality: number): Promise<void> {
  return invoke("update_flashcard_review", { blockId, quality });
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

// Query
export function runQuery(query: string): Promise<Block[]> {
  return invoke("run_query", { queryString: query });
}

// Graph Management
export interface GraphInfo {
  name: string;
  path: string;
}

export function getGraphInfo(): Promise<GraphInfo> {
  return invoke("get_graph_info", {});
}

export function listGraphs(): Promise<GraphInfo[]> {
  return invoke("list_graphs", {});
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
