<script lang="ts">
  import { renderAssistantMarkdown } from "../lib/markdown";

  interface Props {
    title: string;
    content: string;
    loading?: boolean;
    onClose: () => void;
  }

  let { title, content, loading = false, onClose }: Props = $props();

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="help-backdrop" role="presentation" onclick={(event) => {
  if (event.target === event.currentTarget) onClose();
}}>
  <section class="help-dialog" role="dialog" aria-modal="true" aria-labelledby="help-title">
    <header>
      <h1 id="help-title">{title}</h1>
      <button class="help-close" aria-label="Close help" onclick={onClose}>×</button>
    </header>
    {#if loading}
      <p class="help-loading shimmer">Loading help…</p>
    {:else}
      <article class="help-content">
        {@html renderAssistantMarkdown(content)}
      </article>
    {/if}
  </section>
</div>

<style>
  .help-backdrop {
    position: fixed;
    inset: 0;
    z-index: 20000;
    display: grid;
    place-items: center;
    padding: 28px;
    background: color-mix(in srgb, #000 62%, transparent);
  }
  .help-dialog {
    width: min(900px, 100%);
    max-height: min(860px, 90vh);
    overflow: auto;
    border: 1px solid var(--border-color);
    border-radius: 12px;
    background: var(--bg-primary);
    color: var(--text-primary);
    box-shadow: 0 24px 80px color-mix(in srgb, #000 55%, transparent);
  }
  header {
    position: sticky;
    top: 0;
    z-index: 1;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 18px 24px;
    border-bottom: 1px solid var(--border-color);
    background: var(--bg-primary);
  }
  h1 { margin: 0; font-size: 22px; }
  .help-close {
    border: 0;
    background: transparent;
    color: var(--text-secondary);
    font-size: 28px;
    line-height: 1;
    cursor: pointer;
  }
  .help-content { padding: 22px 28px 36px; line-height: 1.55; }
  .help-content :global(h2) { margin-top: 1.6em; }
  .help-content :global(code) { color: var(--accent-color); }
  .help-content :global(table) { border-collapse: collapse; }
  .help-content :global(th), .help-content :global(td) {
    padding: 6px 10px;
    border: 1px solid var(--border-color);
  }
  .help-loading { padding: 28px; color: var(--text-secondary); }
</style>
