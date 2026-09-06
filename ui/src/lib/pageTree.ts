import { invoke } from "@tauri-apps/api/core";
import type { PageTreeViewNode } from "./pageTreeState";

// The contract's Rust payloads use serde's default snake_case. If core opts
// into camelCase later, this is the only value that changes.
export type PayloadCase = "snake" | "camel";
export const PAYLOAD_CASE: PayloadCase = "snake";

function snakeToCamelKey(key: string): string {
  return key.replace(/_([a-z0-9])/g, (_match, character: string) => character.toUpperCase());
}

function camelToSnakeKey(key: string): string {
  return key.replace(/[A-Z]/g, (character) => `_${character.toLowerCase()}`);
}

function mapKeysDeep(value: unknown, mapKey: (key: string) => string): unknown {
  if (value === null || typeof value !== "object") return value;

  type Container = unknown[] | Record<string, unknown>;
  const root: Container = Array.isArray(value) ? [] : {};
  const pending: Array<{ source: Container; target: Container }> = [{
    source: value as Container,
    target: root,
  }];

  while (pending.length > 0) {
    const { source, target } = pending.pop()!;
    const entries = Array.isArray(source)
      ? source.map((child, index) => [String(index), child] as const)
      : Object.entries(source);
    for (const [key, child] of entries) {
      const targetKey = Array.isArray(target) ? Number(key) : mapKey(key);
      if (child !== null && typeof child === "object") {
        const next: Container = Array.isArray(child) ? [] : {};
        (target as Record<string | number, unknown>)[targetKey] = next;
        pending.push({ source: child as Container, target: next });
      } else {
        (target as Record<string | number, unknown>)[targetKey] = child;
      }
    }
  }
  return root;
}

export function snakeToCamelDeep<T = unknown>(value: unknown): T {
  return mapKeysDeep(value, snakeToCamelKey) as T;
}

export function camelToSnakeDeep<T = unknown>(value: unknown): T {
  return mapKeysDeep(value, camelToSnakeKey) as T;
}

function fromWire<T>(value: unknown): T {
  return PAYLOAD_CASE === "camel" ? camelToSnakeDeep<T>(value) : value as T;
}

export function isCommandNotRegistered(error: unknown): boolean {
  const message = String(error).toLowerCase();
  return (
    (message.includes("command") && message.includes("not found"))
    || message.includes("not allowed")
    || message.includes("not registered")
    || message.includes("unknown command")
  );
}

export interface CommandResult<T> {
  available: boolean;
  value: T;
}

export async function withMissingCommandFallback<T>(
  operation: () => Promise<T>,
  fallback: T,
): Promise<CommandResult<T>> {
  try {
    return { available: true, value: await operation() };
  } catch (error) {
    if (!isCommandNotRegistered(error)) throw error;
    return { available: false, value: fallback };
  }
}

// Contract-backed types and wrappers are kept below the wire codec so casing
// conversion stays impossible to bypass at individual call sites.
export type PageTreeSource = "namespace" | "tags";

export interface TreeNode {
  key: string;
  label: string;
  page_id: string | null;
  children: TreeNode[];
  descendant_count: number;
}

export interface CollectionSummary {
  id: string;
  title: string;
  kind: string;
  member_count: number;
}

export interface CollectionMember {
  block_id: string;
  order_index: number;
  page_title: string;
}

export async function pagesNamespaceTree(): Promise<TreeNode[]> {
  const raw = await invoke("pages_namespace_tree");
  return fromWire<TreeNode[]>(raw);
}

export async function pagesTagTree(): Promise<TreeNode[]> {
  const raw = await invoke("pages_tag_tree");
  return fromWire<TreeNode[]>(raw);
}

export function pageSetCollection(pageId: string, kind: string | null): Promise<void> {
  return invoke("page_set_collection", { pageId, kind });
}

export async function pagesListCollections(): Promise<CollectionSummary[]> {
  const raw = await invoke("pages_list_collections");
  return fromWire<CollectionSummary[]>(raw);
}

export function getPageTree(source: PageTreeSource): Promise<TreeNode[]> {
  return source === "namespace" ? pagesNamespaceTree() : pagesTagTree();
}

/**
 * Read a page's collection kind from its properties.
 *
 * The marker is a **flat string** (`collection:: book`), not a nested object.
 * That shape is forced by persistence: the markdown serializer only writes
 * string properties, and indexing a file replaces a page's properties with
 * whatever the parser read back — so a nested marker was written to the
 * database and then silently erased by the next reindex or sync pull. See
 * `core/src/knowledge/collections.rs`; this must stay byte-compatible with
 * `collection_of` there, since both decode the same wire data.
 */
export function getCollectionKind(properties: unknown): string | null {
  if (!properties || typeof properties !== "object" || Array.isArray(properties)) return null;
  const marker = (properties as Record<string, unknown>).collection;
  if (typeof marker !== "string") return null;
  const kind = marker.trim();
  return kind === "" ? null : kind;
}

/**
 * A collection member is one ordered block carrying at least one page link.
 * Multi-link prose is still one member in core, so its first link is the
 * deterministic navigation target rather than inflating the displayed count.
 */
export function collectionMembersFromBlocks(
  blocks: readonly { id: string; order_index: number; content: string }[],
): CollectionMember[] {
  const members: CollectionMember[] = [];
  for (const block of blocks) {
    const match = /\[\[([^\]]+)\]\]/.exec(block.content);
    if (!match) continue;
    members.push({
      block_id: block.id,
      order_index: block.order_index,
      page_title: match[1].replace(/\\/g, "/"),
    });
  }
  return members;
}

export function pageTreeReferencesChanged(previous: string, next: string): boolean {
  const before = collectTreeReferences(previous);
  const after = collectTreeReferences(next);
  if (before.size !== after.size) return true;
  for (const reference of before) {
    if (!after.has(reference)) return true;
  }
  return false;
}

function collectTreeReferences(content: string): Set<string> {
  const references = new Set<string>();
  for (const match of content.matchAll(/\[\[([^\]]+)\]\]/g)) {
    references.add(`page:${match[1].replace(/\\/g, "/").toLowerCase()}`);
  }
  for (const match of content.matchAll(/#([a-zA-Z0-9_/\\-]+)/g)) {
    if (match[1] === "flashcard") continue;
    references.add(`tag:${match[1].replace(/\\/g, "/").toLowerCase()}`);
  }
  return references;
}

export function toPageTreeView(
  nodes: readonly TreeNode[],
  source: PageTreeSource,
): PageTreeViewNode[] {
  const output: PageTreeViewNode[] = [];
  const pending: Array<{
    sourceNode: TreeNode;
    target: PageTreeViewNode[];
  }> = [];

  for (let index = nodes.length - 1; index >= 0; index -= 1) {
    pending.push({ sourceNode: nodes[index], target: output });
  }

  while (pending.length > 0) {
    const { sourceNode, target } = pending.pop()!;
    const viewNode: PageTreeViewNode = {
      id: `${source}:${sourceNode.key}`,
      label: sourceNode.label,
      page_id: sourceNode.page_id,
      page_title: sourceNode.page_id ? sourceNode.key : null,
      count: sourceNode.descendant_count,
      children: [],
    };
    target.push(viewNode);

    for (let index = sourceNode.children.length - 1; index >= 0; index -= 1) {
      pending.push({
        sourceNode: sourceNode.children[index],
        target: viewNode.children,
      });
    }
  }

  return output;
}
