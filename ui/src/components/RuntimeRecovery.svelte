<script lang="ts">
  import type { RuntimeRecovery } from "../lib/knowledge";
  let { records, busy = false, onRetry }: {
    records: RuntimeRecovery[];
    busy?: boolean;
    onRetry: (key: string) => void | Promise<void>;
  } = $props();
</script>

{#if records.length}
  <section aria-label="Native model recovery" data-help-context="ai">
    <h4>Native model recovery</h4>
    <p>An unconfirmed native exit disables automatic GPU attempts. Recovery is retained until the worker exits cleanly; CPU remains available when RAM permits.</p>
    {#each records as recovery (recovery.key)}
      <p><strong>{recovery.label}</strong>: {recovery.reason}</p>
      <button disabled={busy} onclick={() => onRetry(recovery.key)}>Allow one GPU attempt</button>
    {/each}
  </section>
{/if}
