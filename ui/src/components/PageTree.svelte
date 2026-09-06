<script lang="ts">
  import { tick } from "svelte";
  import {
    collectBranchIds,
    findAncestorIdsForPage,
    flattenVisibleTree,
    groupRowsByRoot,
    loadExpansionState,
    pruneExpansionState,
    reduceTreeNavigation,
    saveExpansionState,
    type PageTreeViewNode,
  } from "../lib/pageTreeState";

  interface Props {
    nodes: PageTreeViewNode[];
    onNavigate: (title: string) => void;
    selectedPageId?: string | null;
    storageKey?: string;
    ariaLabel?: string;
    density?: "compact" | "comfortable";
    emptyText?: string;
    /**
     * Active filter text, or "" when unfiltered.
     *
     * While filtering, every branch is revealed so matches are actually
     * visible: the filter keeps a match's ancestors to give it context, and
     * without revealing them the match sits hidden inside a collapsed folder
     * and the search looks like it found nothing.
     */
    revealToken?: string;
    /** Flow top-level branches into as many columns as the width allows. */
    columns?: boolean;
    onPageContextMenu?: (event: MouseEvent, node: PageTreeViewNode) => void;
  }

  let {
    nodes,
    onNavigate,
    selectedPageId = null,
    storageKey,
    ariaLabel = "Pages",
    density = "comfortable",
    emptyText = "No pages in this tree.",
    revealToken = "",
    columns = false,
    onPageContextMenu,
  }: Props = $props();

  let expanded = $state<Set<string>>(new Set());
  let focusedId = $state<string | null>(null);
  let revealedPageId = $state<string | null>(null);
  let loadedStorageKey = $state<string | null>(null);
  const itemElements = new Map<string, HTMLButtonElement>();

  const revealing = $derived(revealToken !== "");

  /**
   * Branches collapsed by hand during the current reveal.
   *
   * Tagged with the filter text it belongs to rather than cleared by an
   * effect, so it lapses on its own when the filter changes — no effect that
   * writes state it also reads, which is what froze this view once already.
   */
  let handCollapsed = $state<{ token: string; ids: Set<string> }>({
    token: "",
    ids: new Set(),
  });

  /** What the tree actually draws: the reader's own state, or a full reveal. */
  const effectiveExpanded = $derived.by(() => {
    if (!revealing) return expanded;
    const shown = new Set(branchIds);
    if (handCollapsed.token === revealToken) {
      for (const id of handCollapsed.ids) shown.delete(id);
    }
    return shown;
  });

  const visibleRows = $derived(
    flattenVisibleTree(nodes, effectiveExpanded, (node) => node.id),
  );
  const branchIds = $derived(
    collectBranchIds(nodes, (node) => node.id),
  );

  // Grouped so a folder is never split from its children by a column break.
  const rowGroups = $derived(groupRowsByRoot(visibleRows));

  $effect(() => {
    const key = storageKey ?? null;
    if (loadedStorageKey === key) return;
    expanded = key
      ? loadExpansionState(
          typeof window === "undefined" ? null : window.localStorage,
          key,
        )
      : new Set();
    loadedStorageKey = key;
  });

  $effect(() => {
    if (!storageKey || loadedStorageKey !== storageKey) return;
    saveExpansionState(
      typeof window === "undefined" ? null : window.localStorage,
      storageKey,
      expanded,
    );
  });

  // Drop expansion state for branches that no longer exist.
  //
  // Deliberately idempotent rather than guarded by "have I seen this array
  // before": pruning an already-pruned set changes nothing, so the effect
  // settles after one pass on its own. The previous version compared array
  // identity, which silently depended on the caller passing the *same* array
  // back every time — the moment one sorted the tree (a new array each render)
  // the guard never matched, the effect wrote state it also read, and Svelte
  // aborted with `effect_update_depth_exceeded`, freezing the whole view.
  $effect(() => {
    // Never while filtering. `branchIds` comes from the *filtered* tree, so
    // pruning then would read every branch the filter hid as deleted and
    // persist their removal — expand two folders, search, clear the search,
    // and the folder you were not searching for is collapsed for good.
    if (revealing || nodes.length === 0) return;
    const next = pruneExpansionState(expanded, branchIds);
    if (next.size !== expanded.size) expanded = next;
  });

  $effect(() => {
    const pageId = selectedPageId;
    // Keyed on the page alone. Including the node array meant re-revealing
    // whenever the tree was rebuilt, which also re-expanded ancestors the
    // reader had deliberately collapsed.
    if (!pageId || revealedPageId === pageId || nodes.length === 0) return;
    const ancestors = findAncestorIdsForPage(
      nodes,
      pageId,
      (node) => node.id,
      (node) => node.page_id,
    );
    if (ancestors === null) return;
    revealedPageId = pageId;
    if (ancestors.length === 0) return;
    const next = new Set(expanded);
    for (const id of ancestors) next.add(id);
    if (next.size !== expanded.size) expanded = next;
  });

  $effect(() => {
    const rows = visibleRows;
    if (rows.length === 0) {
      focusedId = null;
      return;
    }
    if (focusedId && rows.some((row) => row.id === focusedId)) return;
    focusedId =
      rows.find((row) => row.node.page_id === selectedPageId)?.id
      ?? rows[0].id;
  });

  function registerTreeItem(element: HTMLButtonElement, id: string) {
    let currentId = id;
    itemElements.set(currentId, element);
    return {
      update(nextId: string) {
        itemElements.delete(currentId);
        currentId = nextId;
        itemElements.set(currentId, element);
      },
      destroy() {
        itemElements.delete(currentId);
      },
    };
  }

  function setExpanded(id: string, shouldExpand: boolean) {
    // While filtering, a collapse applies to the reveal only. Writing it to the
    // reader's own expansion state would let a search quietly rearrange the
    // tree they come back to once the filter is cleared.
    if (revealing) {
      const ids = new Set(handCollapsed.token === revealToken ? handCollapsed.ids : []);
      if (shouldExpand) ids.delete(id);
      else ids.add(id);
      handCollapsed = { token: revealToken, ids };
      return;
    }
    const next = new Set(expanded);
    if (shouldExpand) next.add(id);
    else next.delete(id);
    expanded = next;
  }

  function toggleExpanded(id: string) {
    setExpanded(id, !effectiveExpanded.has(id));
  }

  function activateNode(node: PageTreeViewNode) {
    if (node.page_id && node.page_title) {
      onNavigate(node.page_title);
      return;
    }
    if (node.children.length > 0) toggleExpanded(node.id);
  }

  function handleNodeClick(
    event: MouseEvent,
    node: PageTreeViewNode,
    id: string,
    hasChildren: boolean,
  ) {
    focusedId = id;
    const target = event.target as Element | null;
    if (hasChildren && target?.closest("[data-disclosure]")) {
      toggleExpanded(id);
      return;
    }
    activateNode(node);
  }

  async function handleTreeKeydown(event: KeyboardEvent) {
    const result = reduceTreeNavigation(
      event.key,
      visibleRows.map((row) => ({
        id: row.id,
        parent_id: row.parent_id,
        has_children: row.has_children,
        can_activate: row.node.page_id !== null && row.node.page_title !== null,
      })),
      focusedId,
      expanded,
    );
    if (!result.handled) return;
    event.preventDefault();

    const current = visibleRows.find((row) => row.id === focusedId);
    if (current && result.expansion) {
      if (result.expansion === "expand") setExpanded(current.id, true);
      else if (result.expansion === "collapse") setExpanded(current.id, false);
      else toggleExpanded(current.id);
    }
    if (current && result.activate) activateNode(current.node);

    focusedId = result.focus_id;
    await tick();
    if (focusedId) itemElements.get(focusedId)?.focus();
  }

  function handleContextMenu(event: MouseEvent, node: PageTreeViewNode) {
    if (!node.page_id || !onPageContextMenu) return;
    event.preventDefault();
    event.stopPropagation();
    onPageContextMenu(event, node);
  }
</script>

<section class="tree-shell" class:compact={density === "compact"}>
  {#if nodes.length === 0}
    <p class="tree-empty">{emptyText}</p>
  {:else}
    <div
      class="tree"
      class:columns
      role="tree"
      aria-label={ariaLabel}
      tabindex="-1"
      onkeydown={handleTreeKeydown}
    >
      {#each rowGroups as group (group[0].id)}
        <div class="tree-group" role="none">
          {#each group as row (row.id)}
            <div
              class="tree-row"
              role="none"
              style={`--tree-depth: ${Math.min(row.level - 1, 12)}`}
            >
              <button
                type="button"
                class="tree-item"
                class:grouping={row.node.page_id === null}
                class:active={row.node.page_id !== null && row.node.page_id === selectedPageId}
                role="treeitem"
                aria-level={row.level}
                aria-posinset={row.position}
                aria-setsize={row.set_size}
                aria-expanded={row.has_children ? effectiveExpanded.has(row.id) : undefined}
                aria-selected={row.node.page_id !== null ? row.node.page_id === selectedPageId : undefined}
                tabindex={focusedId === row.id ? 0 : -1}
                use:registerTreeItem={row.id}
                onclick={(event) => handleNodeClick(event, row.node, row.id, row.has_children)}
                onfocus={() => { focusedId = row.id; }}
                oncontextmenu={(event) => handleContextMenu(event, row.node)}
              >
                {#if row.has_children}
                  <span
                    class="disclosure"
                    data-disclosure
                    title={`${effectiveExpanded.has(row.id) ? "Collapse" : "Expand"} ${row.node.label}`}
                    aria-hidden="true"
                  >
                  <svg
                    class:expanded={effectiveExpanded.has(row.id)}
                    width="12"
                    height="12"
                    viewBox="0 0 16 16"
                    fill="none"
                    aria-hidden="true"
                  >
                    <path d="m6 3.5 4.5 4.5L6 12.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
                  </svg>
                  </span>
                {:else}
                  <span class="disclosure-spacer" aria-hidden="true"></span>
                {/if}
                <span class="node-icon" aria-hidden="true">
                  {#if row.node.page_id === null}
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                      <path d="M1.75 4.25h4l1.2 1.5h7.3v6.5a1.5 1.5 0 0 1-1.5 1.5h-11z" stroke="currentColor" stroke-width="1.25" stroke-linejoin="round" />
                      <path d="M1.75 4.25v-1a1 1 0 0 1 1-1h2.4l1.2 1.5h6.4a1.5 1.5 0 0 1 1.5 1.5v.5" stroke="currentColor" stroke-width="1.25" stroke-linejoin="round" />
                    </svg>
                  {:else}
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="none">
                      <path d="M3 1.75h6l4 4v8.5H3z" stroke="currentColor" stroke-width="1.25" stroke-linejoin="round" />
                      <path d="M9 1.75v4h4" stroke="currentColor" stroke-width="1.25" stroke-linejoin="round" />
                    </svg>
                  {/if}
                </span>
                <span class="node-label">{row.node.label}</span>
                <span
                  class="node-count"
                  aria-label={`${row.node.count} ${row.node.count === 1 ? "page" : "pages"}`}
                >
                  {row.node.count}
                </span>
              </button>
            </div>
          {/each}
        </div>
      {/each}
    </div>
  {/if}
</section>

<style>
  .tree-shell {
    min-width: 0;
  }

  /* One column of a few hundred loose pages leaves most of a wide window empty
     and pushes the rest below the fold. Sized in rem rather than as a fixed
     count so the number of columns follows the window instead of fighting it.
     `display: block` is required: a flex container ignores column-width. */
  .tree.columns {
    display: block;
    /* Capped as well as sized: columns read top-to-bottom then left-to-right,
       which stays followable at four but stops being so once a listing is
       spread across six or seven. The cap also holds if the container is ever
       widened or the reader's font size is small. */
    columns: 4 22rem;
    column-gap: 28px;
  }

  /* Columns break wherever they run out of room, so each branch is kept whole
     to stop a folder being separated from its children. */
  .tree.columns .tree-group {
    break-inside: avoid;
    margin-bottom: 2px;
  }

  .tree-item:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }


  .tree {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  /* Rows sit inside a per-branch wrapper, so their spacing lives here rather
     than on `.tree`, whose gap now falls between whole branches. */
  .tree-group {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .tree-row {
    padding-left: calc(var(--tree-depth) * 16px);
  }

  .disclosure,
  .disclosure-spacer {
    width: 20px;
    height: 20px;
    flex: 0 0 auto;
  }

  .disclosure {
    display: grid;
    place-items: center;
    padding: 0;
    border-radius: 4px;
    color: var(--text-muted);
  }

  .tree-item:hover .disclosure {
    color: var(--text-primary);
  }

  .disclosure svg {
    transition: transform 120ms ease-out;
  }

  .disclosure svg.expanded {
    transform: rotate(90deg);
  }

  .tree-item {
    display: flex;
    align-items: center;
    width: 100%;
    min-width: 0;
    min-height: 32px;
    gap: 7px;
    padding: 4px 7px 4px 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    /* Actionable text, so it takes the primary token. `--text-secondary` is a
       de-emphasis colour and drops to ~2.9:1 on the light themes, which is
       below AA for 13px — fine for supporting metadata, not for something you
       are meant to read and click. */
    color: var(--text-primary);
    font: inherit;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }

  .tree-item:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .tree-item.active {
    color: var(--text-primary);
    background: var(--bg-active);
  }

  .tree-item.grouping {
    color: var(--text-secondary);
    font-weight: 600;
  }

  .tree-item.grouping:hover {
    color: var(--text-secondary);
  }

  .node-icon {
    display: grid;
    place-items: center;
    flex: 0 0 auto;
    color: var(--text-secondary);
  }

  .tree-item.active .node-icon {
    color: var(--accent);
  }

  .node-label {
    min-width: 0;
    flex: 1;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .node-count {
    flex: 0 0 auto;
    min-width: 20px;
    padding: 1px 5px;
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--text-secondary);
    background: var(--bg-secondary);
    font-size: 10px;
    font-variant-numeric: tabular-nums;
    text-align: center;
  }

  .tree-empty {
    margin: 0;
    padding: 14px 8px;
    color: var(--text-secondary);
    font-size: 12px;
  }

  .compact .tree-row {
    padding-left: calc(var(--tree-depth) * 12px);
  }

  .compact .disclosure,
  .compact .disclosure-spacer {
    width: 18px;
    height: 18px;
  }

  .compact .tree-item {
    min-height: 28px;
    padding: 3px 5px 3px 0;
    gap: 5px;
    font-size: 12px;
  }

  @media (prefers-reduced-motion: reduce) {
    .disclosure svg {
      transition: none;
    }
  }
</style>
