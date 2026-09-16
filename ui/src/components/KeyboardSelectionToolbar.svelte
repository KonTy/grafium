<script lang="ts">
  import type { KeyboardBlockSelection } from "../lib/keyboardBlockSelection.svelte";
  let { selection }: { selection: KeyboardBlockSelection } = $props();
</script>

{#if selection.active}
  <div class="keyboard-selection-toolbar" role="toolbar" aria-label="Selected blocks">
    <span>{selection.count} blocks selected{selection.pageCount > 1 ? ` across ${selection.pageCount} days` : ""}</span>
    <button type="button" disabled={selection.busy} onclick={selection.copy}>Copy</button>
    <button type="button" disabled={selection.busy} onclick={selection.remove}>Delete</button>
    <button type="button" disabled={selection.busy} onclick={selection.clear}>Clear</button>
    {#if selection.busy}<span role="status">Saving...</span>{/if}
  </div>
{/if}

<style>
  .keyboard-selection-toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    color: var(--text-primary);
    background: var(--bg-secondary);
    border-bottom: 1px solid var(--border);
    font-size: 13px;
  }
  button {
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 4px 8px;
    background: var(--bg-hover);
    color: var(--text-primary);
    cursor: pointer;
  }
  button:focus-visible { outline: 2px solid var(--accent); }
  button:disabled { opacity: 0.5; cursor: default; }
</style>
