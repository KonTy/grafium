export interface TreeNodeLike<TNode extends TreeNodeLike<TNode>> {
  children: TNode[];
}

export interface PageTreeViewNode extends TreeNodeLike<PageTreeViewNode> {
  id: string;
  label: string;
  page_id: string | null;
  page_title: string | null;
  count: number;
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

export function saveExpansionState(
  storage: StorageLike | null | undefined,
  key: string,
  expanded: ReadonlySet<string>,
): boolean {
  if (!storage) return false;
  const payload: PersistedExpansion = {
    version: 1,
    expanded: Array.from(expanded).sort(),
  };
  try {
    storage.setItem(key, JSON.stringify(payload));
    return true;
  } catch {
    return false;
  }
}

function isPersistedExpansion(value: unknown): value is PersistedExpansion {
  if (value === null || typeof value !== "object") return false;
  const candidate = value as Partial<PersistedExpansion>;
  return candidate.version === 1 && Array.isArray(candidate.expanded);
}
