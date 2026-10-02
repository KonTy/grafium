<script lang="ts">
  import { hasLimitedMemoryProtection } from "../lib/modelRecovery";
  let { warnings = [], details = [] }: { warnings?: string[]; details?: string[] } = $props();
  const lines = $derived([...new Set([...warnings, ...details].filter(Boolean))]);
</script>

{#if hasLimitedMemoryProtection(warnings)}
  <p class="memory-warning">Memory protection is limited on this computer.</p>
{/if}
{#if lines.length}
  <details class="runtime-details">
    <summary>Technical details</summary>
    {#each lines as line}<p>{line}</p>{/each}
  </details>
{/if}

<style>
  .memory-warning { color: var(--warning, #a76d16); margin: 0; }
  .runtime-details { width: 100%; font-size: 12px; color: var(--text-secondary); }
  summary { cursor: pointer; }
  summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  p { line-height: 1.5; overflow-wrap: anywhere; }
</style>
