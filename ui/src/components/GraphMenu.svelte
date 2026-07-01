<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    getGraphInfo,
    listGraphs,
    openGraph,
    validateGraph,
    createGraph,
    reindexCurrent,
    removeGraph,
    type GraphInfo,
  } from "../lib/api";

  interface Props {
    onGraphChanged: () => void;
  }

  let { onGraphChanged }: Props = $props();

  let currentGraph: GraphInfo | null = $state(null);
  let allGraphs: GraphInfo[] = $state([]);
  let menuOpen = $state(false);
  let showCreateDialog = $state(false);
  let newGraphName = $state("");
  let isLoading = $state(false);

  $effect(() => {
    loadGraphInfo();
  });

  async function loadGraphInfo() {
    try {
      currentGraph = await getGraphInfo();
      allGraphs = await listGraphs();
    } catch (e) {
      console.error("Failed to load graph info:", e);
    }
  }

  function toggleMenu() {
    menuOpen = !menuOpen;
  }

  function closeMenu() {
    menuOpen = false;
  }

  async function handleOpenExisting() {
    closeMenu();
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Open Graph Folder",
    });

    if (selected && typeof selected === "string") {
      isLoading = true;
      try {
        const report = await validateGraph(selected);
        if (!report.is_valid) {
          const missing = [
            !report.has_pages_dir    && "pages/",
            !report.has_journals_dir && "journals/",
            !report.has_metadata_dir && "metadata/",
            !report.has_valid_db     && "metadata/index.db (corrupted)",
          ].filter(Boolean).join(", ");
          alert(
            `"${selected}" is not a valid Grafium graph.\n\n` +
            `Missing: ${missing}\n\n` +
            `To create a new graph here, use New Graph instead.`
          );
          return;
        }
        await openGraph(selected);
        await loadGraphInfo();
        onGraphChanged();
      } catch (e) {
        console.error("Failed to open graph:", e);
        alert("Failed to open graph: " + e);
      } finally {
        isLoading = false;
      }
    }
  }

  async function handleCreateNew() {
    closeMenu();
    showCreateDialog = true;
    newGraphName = "";
  }

  async function confirmCreate() {
    if (!newGraphName.trim()) return;

    const selected = await open({
      directory: true,
      multiple: false,
      title: "Choose location for new graph",
    });

    if (selected && typeof selected === "string") {
      const graphPath = selected + "/" + newGraphName.trim();
      isLoading = true;
      showCreateDialog = false;
      try {
        await createGraph(graphPath, newGraphName.trim());
        await loadGraphInfo();
        onGraphChanged();
      } catch (e) {
        console.error("Failed to create graph:", e);
        alert("Failed to create graph: " + e);
      } finally {
        isLoading = false;
      }
    }
  }

  async function handleReindex() {
    closeMenu();
    isLoading = true;
    try {
      await reindexCurrent();
      onGraphChanged();
    } catch (e) {
      console.error("Re-index failed:", e);
      alert("Re-index failed: " + e);
    } finally {
      isLoading = false;
    }
  }

  async function handleSwitchGraph(graph: GraphInfo) {
    closeMenu();
    if (graph.path === currentGraph?.path) return;
    isLoading = true;
    try {
      const report = await validateGraph(graph.path);
      if (!report.is_valid) {
        alert(
          `"${graph.name}" can no longer be opened.\n\n` +
          (report.error_message ?? "The graph directory is missing required structure.") +
          `\n\nYou can remove it from the graph list below.`
        );
        return;
      }
      await openGraph(graph.path);
      await loadGraphInfo();
      onGraphChanged();
    } catch (e) {
      console.error("Failed to switch graph:", e);
      alert("Failed to switch graph: " + e);
    } finally {
      isLoading = false;
    }
  }

  function cancelCreate() {
    showCreateDialog = false;
    newGraphName = "";
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      closeMenu();
      cancelCreate();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="graph-menu-container">
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="graph-selector" onclick={toggleMenu}>
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <circle cx="12" cy="12" r="3"></circle>
      <circle cx="4" cy="4" r="2"></circle>
      <circle cx="20" cy="4" r="2"></circle>
      <circle cx="4" cy="20" r="2"></circle>
      <circle cx="20" cy="20" r="2"></circle>
      <line x1="6" y1="6" x2="10" y2="10"></line>
      <line x1="18" y1="6" x2="14" y2="10"></line>
      <line x1="6" y1="18" x2="10" y2="14"></line>
      <line x1="18" y1="18" x2="14" y2="14"></line>
    </svg>
    <span class="graph-name">{isLoading ? "Loading..." : currentGraph?.name ?? "No Graph"}</span>
    <svg class="chevron" class:open={menuOpen} width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
      <polyline points="6 9 12 15 18 9"></polyline>
    </svg>
  </div>

  {#if menuOpen}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="menu-backdrop" onclick={closeMenu}></div>
    <div class="menu-dropdown">
      <div class="menu-section">
        <div class="menu-label">Graphs</div>
        {#each allGraphs as graph}
          <button
            class="menu-item"
            class:active={graph.path === currentGraph?.path}
            onclick={() => handleSwitchGraph(graph)}
          >
            <span class="menu-item-name">{graph.name}</span>
            {#if graph.path === currentGraph?.path}
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <polyline points="20 6 9 17 4 12"></polyline>
              </svg>
            {/if}
          </button>
        {/each}
      </div>
      <div class="menu-divider"></div>
      <button class="menu-item" onclick={handleCreateNew}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <line x1="12" y1="5" x2="12" y2="19"></line>
          <line x1="5" y1="12" x2="19" y2="12"></line>
        </svg>
        <span>New Graph</span>
      </button>
      <button class="menu-item" onclick={handleOpenExisting}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
        </svg>
        <span>Open Folder</span>
      </button>
      <div class="menu-divider"></div>
      <button class="menu-item" onclick={handleReindex}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <polyline points="23 4 23 10 17 10"></polyline>
          <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"></path>
        </svg>
        <span>Re-index</span>
      </button>
    </div>
  {/if}
</div>

{#if showCreateDialog}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="dialog-backdrop" onclick={cancelCreate}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="dialog" onclick={(e) => e.stopPropagation()}>
      <h3 class="dialog-title">Create New Graph</h3>
      <p class="dialog-desc">Choose a name for your new graph. You'll then pick a folder location.</p>
      <input
        type="text"
        class="dialog-input"
        placeholder="Graph name..."
        bind:value={newGraphName}
        onkeydown={(e) => { if (e.key === "Enter") confirmCreate(); }}
      />
      <div class="dialog-actions">
        <button class="btn btn-secondary" onclick={cancelCreate}>Cancel</button>
        <button class="btn btn-primary" onclick={confirmCreate} disabled={!newGraphName.trim()}>
          Choose Location & Create
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .graph-menu-container {
    position: relative;
  }

  .graph-selector {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .graph-selector:hover {
    background: var(--bg-hover);
  }

  .graph-name {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chevron {
    color: var(--text-muted);
    transition: transform 0.2s;
  }

  .chevron.open {
    transform: rotate(180deg);
  }

  .menu-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 99;
  }

  .menu-dropdown {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    margin-top: 4px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    z-index: 100;
    padding: 4px;
    min-width: 220px;
  }

  .menu-section {
    padding: 4px 0;
  }

  .menu-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    padding: 4px 10px 6px;
  }

  .menu-divider {
    height: 1px;
    background: var(--border);
    margin: 4px 0;
  }

  .menu-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 10px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-secondary);
    font-size: 13px;
    cursor: pointer;
    text-align: left;
  }

  .menu-item:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .menu-item.active {
    color: var(--accent);
  }

  .menu-item-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .dialog-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
  }

  .dialog {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 24px;
    width: 400px;
    max-width: 90vw;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  }

  .dialog-title {
    font-size: 18px;
    font-weight: 700;
    color: var(--text-primary);
    margin-bottom: 8px;
  }

  .dialog-desc {
    font-size: 13px;
    color: var(--text-secondary);
    margin-bottom: 16px;
    line-height: 1.5;
  }

  .dialog-input {
    width: 100%;
    padding: 10px 12px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: 14px;
    outline: none;
    margin-bottom: 16px;
  }

  .dialog-input:focus {
    border-color: var(--accent);
  }

  .dialog-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .btn {
    padding: 8px 16px;
    border-radius: 6px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    border: none;
    transition: all 0.15s;
  }

  .btn-secondary {
    background: var(--bg-hover);
    color: var(--text-secondary);
  }

  .btn-secondary:hover {
    background: var(--border);
    color: var(--text-primary);
  }

  .btn-primary {
    background: var(--accent);
    color: #fff;
  }

  .btn-primary:hover {
    opacity: 0.9;
  }

  .btn-primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
