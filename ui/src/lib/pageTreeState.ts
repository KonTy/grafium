import { fuzzyMatches } from "./fuzzy";
export interface TreeNodeLike<TNode extends TreeNodeLike<TNode>> {
  children: TNode[];
}

export interface PageTreeViewNode extends TreeNodeLike<PageTreeViewNode> {
  id: string;
  label: string;
  page_id: string | null;
  page_title: string | null;
  count: number;
  /** Newest `updated_at` at or below this node, in epoch milliseconds. */
  updated_at: number;
  children: PageTreeViewNode[];
}

export interface FlatTreeNode<TNode> {
  node: TNode;
  id: string;
  parent_id: string | null;
  level: number;
  position: number;
  set_size: number;
  has_children: boolean;
}

interface PendingTreeNode<TNode> {
  node: TNode;
  parent_id: string | null;
  level: number;
  position: number;
  set_size: number;
}

export interface TreeNavigationItem {
  id: string;
  parent_id: string | null;
  has_children: boolean;
  can_activate: boolean;
}

export interface TreeNavigationResult {
  handled: boolean;
  focus_id: string | null;
  expansion: "expand" | "collapse" | "toggle" | null;
  activate: boolean;
}

export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

interface PersistedExpansion {
  version: 1;
  expanded: string[];
}

export const SIDEBAR_TREE_STORAGE_KEY = "grafium.pageTree.sidebar.expanded";
export const ALL_PAGES_TREE_STORAGE_KEY = "grafium.pageTree.allPages.expanded";
export const ALL_PAGES_SORT_STORAGE_KEY = "grafium.pageTree.allPages.sort";

/**
 * Scope a storage key to one graph.
 *
 * Expansion state is a set of page paths, which are meaningful only inside the
 * graph they came from. Sharing one key meant opening graph B pruned graph A's
 * saved paths against B's tree and wrote the result back over the shared key —
 * so every switch quietly destroyed the other graph's expansion state, and a
 * path that happened to exist in both carried across as if it were the same
 * node.
 *
 * The graph's path is its stable identity; it is hashed so the key stays short
 * and doesn't leak a filesystem path into storage.
 */
export function graphScopedKey(baseKey: string, graphPath: string | null | undefined): string {
  if (!graphPath) return baseKey;
  // FNV-1a, matching `tagColor.ts` — deterministic across runs and platforms.
  let hash = 0x811c9dc5;
  for (let i = 0; i < graphPath.length; i++) {
    hash ^= graphPath.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return `${baseKey}.${hash.toString(36)}`;
}

/**
 * Builds the visible projection once per expansion change. Keeping rendering
 * flat avoids recursive Svelte components and repeated subtree walks.
 */
export function flattenVisibleTree<TNode extends TreeNodeLike<TNode>>(
  roots: readonly TNode[],
  expanded: ReadonlySet<string>,
  getId: (node: TNode) => string,
): FlatTreeNode<TNode>[] {
  const result: FlatTreeNode<TNode>[] = [];
  const stack: PendingTreeNode<TNode>[] = [];
  const visited = new Set<string>();

  for (let index = roots.length - 1; index >= 0; index -= 1) {
    stack.push({
      node: roots[index],
      parent_id: null,
      level: 1,
      position: index + 1,
      set_size: roots.length,
    });
  }

  while (stack.length > 0) {
    const current = stack.pop()!;
    const id = getId(current.node);
    if (!id || visited.has(id)) continue;
    visited.add(id);

    const children = current.node.children;
    result.push({
      node: current.node,
      id,
      parent_id: current.parent_id,
      level: current.level,
      position: current.position,
      set_size: current.set_size,
      has_children: children.length > 0,
    });

    if (!expanded.has(id) || children.length === 0) continue;
    for (let index = children.length - 1; index >= 0; index -= 1) {
      stack.push({
        node: children[index],
        parent_id: id,
        level: current.level + 1,
        position: index + 1,
        set_size: children.length,
      });
    }
  }

  return result;
}

export function collectBranchIds<TNode extends TreeNodeLike<TNode>>(
  roots: readonly TNode[],
  getId: (node: TNode) => string,
): Set<string> {
  const branchIds = new Set<string>();
  const visited = new Set<string>();
  const stack = [...roots];

  while (stack.length > 0) {
    const node = stack.pop()!;
    const id = getId(node);
    if (!id || visited.has(id)) continue;
    visited.add(id);
    if (node.children.length > 0) branchIds.add(id);
    for (const child of node.children) stack.push(child);
  }

  return branchIds;
}

export function findAncestorIdsForPage<TNode extends TreeNodeLike<TNode>>(
  roots: readonly TNode[],
  pageId: string,
  getId: (node: TNode) => string,
  getPageId: (node: TNode) => string | null,
): string[] | null {
  const parentById = new Map<string, string | null>();
  const visited = new Set<string>();
  const stack = roots.map((node) => ({ node, parentId: null as string | null }));
  let targetId: string | null = null;

  while (stack.length > 0) {
    const { node, parentId } = stack.pop()!;
    const id = getId(node);
    if (!id || visited.has(id)) continue;
    visited.add(id);
    parentById.set(id, parentId);
    if (getPageId(node) === pageId) {
      targetId = id;
      break;
    }
    for (const child of node.children) {
      stack.push({ node: child, parentId: id });
    }
  }

  if (!targetId) return null;
  const ancestors: string[] = [];
  let parentId = parentById.get(targetId) ?? null;
  while (parentId) {
    ancestors.push(parentId);
    parentId = parentById.get(parentId) ?? null;
  }
  ancestors.reverse();
  return ancestors;
}

/**
 * Drop expansion state for branches that no longer exist.
 *
 * Idempotent, and the tree component depends on that: it prunes from an effect
 * that both reads and writes the expansion set, so pruning an already-pruned
 * set has to be a no-op or the effect never settles and Svelte tears the view
 * down with `effect_update_depth_exceeded`.
 */
export function pruneExpansionState(
  expanded: ReadonlySet<string>,
  branchIds: ReadonlySet<string>,
): Set<string> {
  const next = new Set<string>();
  for (const id of expanded) {
    if (branchIds.has(id)) next.add(id);
  }
  return next;
}

export function reduceTreeNavigation(
  key: string,
  items: readonly TreeNavigationItem[],
  focusedId: string | null,
  expanded: ReadonlySet<string>,
): TreeNavigationResult {
  const idle: TreeNavigationResult = {
    handled: false,
    focus_id: focusedId,
    expansion: null,
    activate: false,
  };
  if (items.length === 0) return idle;

  const currentIndex = Math.max(0, items.findIndex((item) => item.id === focusedId));
  const current = items[currentIndex];

  if (key === "ArrowDown") {
    return {
      ...idle,
      handled: true,
      focus_id: items[Math.min(items.length - 1, currentIndex + 1)].id,
    };
  }
  if (key === "ArrowUp") {
    return {
      ...idle,
      handled: true,
      focus_id: items[Math.max(0, currentIndex - 1)].id,
    };
  }
  if (key === "Home") {
    return { ...idle, handled: true, focus_id: items[0].id };
  }
  if (key === "End") {
    return { ...idle, handled: true, focus_id: items[items.length - 1].id };
  }
  if (key === "ArrowRight") {
    if (!current.has_children) return { ...idle, handled: true, focus_id: current.id };
    if (!expanded.has(current.id)) {
      return {
        ...idle,
        handled: true,
        focus_id: current.id,
        expansion: "expand",
      };
    }
    const next = items[currentIndex + 1];
    return {
      ...idle,
      handled: true,
      focus_id: next?.parent_id === current.id ? next.id : current.id,
    };
  }
  if (key === "ArrowLeft") {
    if (current.has_children && expanded.has(current.id)) {
      return {
        ...idle,
        handled: true,
        focus_id: current.id,
        expansion: "collapse",
      };
    }
    return {
      ...idle,
      handled: true,
      focus_id: current.parent_id ?? current.id,
    };
  }
  if (key === "Enter") {
    return {
      ...idle,
      handled: true,
      focus_id: current.id,
      expansion: current.can_activate || !current.has_children ? null : "toggle",
      activate: current.can_activate,
    };
  }

  return idle;
}

export function loadExpansionState(
  storage: StorageLike | null | undefined,
  key: string,
): Set<string> {
  if (!storage) return new Set();
  try {
    const raw = storage.getItem(key);
    if (!raw) return new Set();
    const parsed = JSON.parse(raw) as unknown;
    const values = Array.isArray(parsed)
      ? parsed
      : isPersistedExpansion(parsed)
        ? parsed.expanded
        : [];
    return new Set(values.filter((value): value is string => typeof value === "string" && value.length > 0));
  } catch {
    return new Set();
  }
}

/**
 * Most expanded paths to persist.
 *
 * Each entry is a full path, so cost is proportional to path *length*, not
 * just count: a legitimately deep title expands into one entry per level, and
 * a 5000-level path alone serializes to tens of megabytes — far past any
 * localStorage quota, so the write fails and nothing persists at all. Capping
 * keeps a pathological graph from costing every other graph its saved state.
 */
export const MAX_PERSISTED_EXPANSIONS = 2_000;

/// Longest single path persisted. A path this deep is not something anyone is
/// navigating by hand; dropping it costs a restored expansion, not data.
const MAX_PERSISTED_PATH_LENGTH = 512;

export function saveExpansionState(
  storage: StorageLike | null | undefined,
  key: string,
  expanded: ReadonlySet<string>,
): boolean {
  if (!storage) return false;
  // Shortest-first, so a budget spent on shallow paths restores the parts of
  // the tree a person actually sees rather than one absurd branch.
  const entries = Array.from(expanded)
    .filter((path) => path.length <= MAX_PERSISTED_PATH_LENGTH)
    .sort((a, b) => a.length - b.length || a.localeCompare(b))
    .slice(0, MAX_PERSISTED_EXPANSIONS)
    .sort();

  const payload: PersistedExpansion = { version: 1, expanded: entries };
  try {
    storage.setItem(key, JSON.stringify(payload));
    return true;
  } catch {
    // Quota exhausted or storage denied. Reporting failure lets the caller
    // stop retrying on every subsequent toggle instead of throwing at the
    // same wall repeatedly.
    return false;
  }
}

function isPersistedExpansion(value: unknown): value is PersistedExpansion {
  if (value === null || typeof value !== "object") return false;
  const candidate = value as Partial<PersistedExpansion>;
  return candidate.version === 1 && Array.isArray(candidate.expanded);
}

/**
 * Filter a tree to nodes matching `query`, keeping the path to each match.
 *
 * A match deep in a namespace is meaningless without its ancestors — showing a
 * bare `arc` gives no clue it lives under `mybooks/thingsilove` — so an
 * ancestor is kept whenever any descendant matches, even if the ancestor
 * itself doesn't. A node that matches keeps its whole subtree, since once
 * you've found `tech/linux` you want to see what's inside it.
 *
 * Returns the tree unchanged for an empty query, so an empty search box costs
 * nothing.
 */
export function filterTreeByQuery(
  nodes: readonly PageTreeViewNode[],
  query: string,
): PageTreeViewNode[] {
  if (query.trim() === "") return nodes as PageTreeViewNode[];

  const walk = (list: readonly PageTreeViewNode[]): PageTreeViewNode[] => {
    const kept: PageTreeViewNode[] = [];
    for (const node of list) {
      // Match against the full path, not just the label: typing "linux net"
      // should find `tech/linux/networking` even though no single segment
      // contains both words.
      const path = node.page_title ?? node.id;
      const selfMatches = fuzzyMatches(path, query) || fuzzyMatches(node.label, query);
      const children = selfMatches ? [...node.children] : walk(node.children);
      if (selfMatches || children.length > 0) {
        kept.push({ ...node, children });
      }
    }
    return kept;
  };

  return walk(nodes);
}

/** Every node key in a tree — used to expand all matches after filtering. */
export function collectAllKeys(nodes: readonly PageTreeViewNode[]): Set<string> {
  const keys = new Set<string>();
  const stack = [...nodes];
  while (stack.length > 0) {
    const node = stack.pop()!;
    keys.add(node.id);
    for (const child of node.children) stack.push(child);
  }
  return keys;
}

/** Count real pages (not grouping nodes) in a tree. */
export function countTreePages(nodes: readonly PageTreeViewNode[]): number {
  let count = 0;
  const stack = [...nodes];
  while (stack.length > 0) {
    const node = stack.pop()!;
    if (node.page_id) count++;
    for (const child of node.children) stack.push(child);
  }
  return count;
}

/**
 * Group already-flattened rows so each top-level branch stays with its
 * descendants.
 *
 * Only the column layout needs this, and it is the whole reason that layout is
 * safe: CSS columns break wherever they run out of room, so without grouping a
 * folder could sit at the foot of one column with its children stranded at the
 * head of the next.
 *
 * Rows arrive in tree order, so a new group starts at every depth-1 row. Rows
 * appearing before the first depth-1 row — which should not happen, but would
 * silently drop pages if it did — are kept in a leading group rather than
 * discarded.
 */
export function groupRowsByRoot<TNode>(
  rows: readonly FlatTreeNode<TNode>[],
): FlatTreeNode<TNode>[][] {
  const groups: FlatTreeNode<TNode>[][] = [];
  for (const row of rows) {
    if (row.level === 1 || groups.length === 0) groups.push([row]);
    else groups[groups.length - 1].push(row);
  }
  return groups;
}

export type PageTreeSortMode = "name" | "recent";

/**
 * Reorder a tree without changing its shape.
 *
 * Folders are always their own group at the top, the way a file manager orders
 * a listing: structure stays put, and the chosen order applies *within* each
 * group rather than shuffling folders in among hundreds of loose pages.
 *
 * The mode decides the order inside a group:
 *
 *   - `name` — alphabetical, case-insensitive.
 *   - `recent` — newest first. A folder carries the date of its newest page,
 *     so a book you edited this morning leads the folders instead of sitting
 *     wherever its name happens to fall.
 *
 * Ties fall back to name so the order never wanders between renders — pages
 * imported together commonly share a timestamp to the millisecond.
 */
export function sortTree(
  nodes: readonly PageTreeViewNode[],
  mode: PageTreeSortMode,
): PageTreeViewNode[] {
  // `sensitivity: "base"` treats `Cafe`/`café` and `Foo`/`foo` as equal, so it
  // cannot order them on its own; the id breaks the remaining tie to give a
  // total order that does not depend on the order rows arrived in.
  const byName = (a: PageTreeViewNode, b: PageTreeViewNode) =>
    a.label.localeCompare(b.label, undefined, { sensitivity: "base" })
    || a.id.localeCompare(b.id);
  const within =
    mode === "recent"
      ? (a: PageTreeViewNode, b: PageTreeViewNode) =>
          b.updated_at - a.updated_at || byName(a, b)
      : byName;

  const compare = (a: PageTreeViewNode, b: PageTreeViewNode) => {
    const aBranch = a.children.length > 0;
    const bBranch = b.children.length > 0;
    if (aBranch !== bBranch) return aBranch ? -1 : 1;
    return within(a, b);
  };

  // Iterative: depth here is a page title's segment count, which is user data,
  // and a deeply nested tag would overflow the stack on the way down.
  const sorted = nodes.map((node) => ({ ...node, children: [...node.children] }));
  const stack: PageTreeViewNode[][] = [sorted];
  while (stack.length > 0) {
    const level = stack.pop()!;
    level.sort(compare);
    for (const node of level) {
      if (node.children.length === 0) continue;
      node.children = node.children.map((child) => ({
        ...child,
        children: [...child.children],
      }));
      stack.push(node.children);
    }
  }
  return sorted;
}
