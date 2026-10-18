<script lang="ts">
  import type { PendingMergeNote } from "../lib/sync";

  let { notes, onOpen }: { notes: PendingMergeNote[]; onOpen: (note: PendingMergeNote) => void } = $props();
</script>

<section aria-label="Notes to merge">
  <p>Edit each marked spot, delete its marker lines, then sync again.</p>
  {#each notes as note (note.rel_path)}
    <div class="entry">
      <button class="item" onclick={() => onOpen(note)} aria-label={`Open ${note.rel_path}`}>
        <strong>{note.rel_path}</strong>
        <span>{note.conflicts === 1 ? "1 spot" : `${note.conflicts} spots`} to merge</span>
      </button>
    </div>
  {/each}
</section>

<style>
  p { margin: 0 0 10px; font-size: 13px; line-height: 1.5; color: var(--text-secondary); overflow-wrap: anywhere; }
  .entry { padding-bottom: 10px; border-bottom: 1px solid var(--border); margin-bottom: 10px; }
  .item { display: flex; flex-direction: column; align-items: flex-start; width: 100%; text-align: left; gap: 2px; background: none; border: none; color: var(--text-secondary); padding: 6px 12px; border-radius: 4px; cursor: pointer; font: inherit; font-size: 13px; }
  .item:hover { background: var(--bg-hover); color: var(--text-primary); }
  .item:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .item strong { overflow-wrap: anywhere; }
  .item span { color: var(--text-secondary); font-size: 11px; }
</style>
