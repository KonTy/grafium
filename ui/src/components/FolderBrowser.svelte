<script lang="ts">
  import { listDirectory, type DirEntry } from "../lib/api";

  interface Props {
    onSelect: (path: string) => void;
    onCancel: () => void;
    title?: string;
  }

  let { onSelect, onCancel, title = "Select Folder" }: Props = $props();

  let currentPath = $state("");
  let entries = $state<DirEntry[]>([]);
  let loading = $state(true);
  let error = $state("");

  // Load on mount
  loadDir("");

  async function loadDir(path: string) {
    loading = true;
    error = "";
    try {
      console.log("[folder-browser] loading:", path);
      const result = await listDirectory(path);
      console.log("[folder-browser] got:", result.current_path, result.entries.length, "entries");
      currentPath = result.current_path;
      entries = result.entries;
    } catch (e) {
      console.error("[folder-browser] error:", e);
      error = String(e);
      entries = [];
    }
    loading = false;
  }

  function navigateTo(entry: DirEntry) {
    loadDir(entry.path);
  }

  function selectCurrent() {
    console.log("[folder-browser] selected:", currentPath);
    onSelect(currentPath);
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="folder-browser-backdrop" onclick={onCancel}>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="folder-browser" onclick={(e) => e.stopPropagation()}>
    <h3 class="fb-title">{title}</h3>
    <div class="fb-path">{currentPath || "Loading..."}</div>

    {#if error}
      <div class="fb-error">{error}</div>
    {/if}

    <div class="fb-list">
      {#if loading}
        <div class="fb-loading">Loading...</div>
      {:else}
        {#each entries as entry}
          <button class="fb-entry" onclick={() => navigateTo(entry)}>
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              {#if entry.name === ".."}
                <polyline points="15 18 9 12 15 6"></polyline>
              {:else}
                <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
              {/if}
            </svg>
            <span class="fb-entry-name">{entry.name}</span>
          </button>
        {/each}
        {#if entries.length === 0 && !error}
          <div class="fb-empty">Empty folder — you can select it</div>
        {/if}
      {/if}
    </div>

    <div class="fb-actions">
      <button class="fb-btn fb-btn-cancel" onclick={onCancel}>Cancel</button>
      <button class="fb-btn fb-btn-select" onclick={selectCurrent} disabled={loading || !!error}>Select This Folder</button>
    </div>
  </div>
</div>

<style>
  .folder-browser-backdrop {
    position: fixed;
    top: 0; left: 0; right: 0; bottom: 0;
    background: rgba(0,0,0,0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 300;
    padding: 16px;
  }

  .folder-browser {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 12px;
    width: 100%;
    max-width: 400px;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .fb-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
    padding: 16px 16px 8px;
  }

  .fb-path {
    font-size: 11px;
    font-family: monospace;
    color: var(--text-muted);
    padding: 0 16px 12px;
    word-break: break-all;
  }

  .fb-error {
    font-size: 12px;
    color: var(--error, #f44);
    padding: 8px 16px;
  }

  .fb-list {
    flex: 1;
    overflow-y: auto;
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
    min-height: 150px;
    max-height: 300px;
  }

  .fb-entry {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 12px 16px;
    background: none;
    border: none;
    border-bottom: 1px solid var(--border);
    color: var(--text-primary);
    font-size: 14px;
    cursor: pointer;
    text-align: left;
    transition: background 0.1s;
  }

  .fb-entry:last-child {
    border-bottom: none;
  }

  .fb-entry:hover, .fb-entry:active {
    background: var(--bg-hover);
  }

  .fb-entry-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .fb-loading, .fb-empty {
    padding: 24px 16px;
    text-align: center;
    color: var(--text-muted);
    font-size: 13px;
  }

  .fb-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 16px;
  }

  .fb-btn {
    padding: 10px 16px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    border: none;
  }

  .fb-btn-cancel {
    background: var(--btn-bg);
    color: var(--text-secondary);
  }

  .fb-btn-cancel:hover {
    background: var(--btn-bg-hover);
  }

  .fb-btn-select {
    background: var(--btn-primary-bg);
    color: var(--btn-primary-fg);
  }

  .fb-btn-select:hover {
    background: var(--btn-primary-hover);
  }
</style>
