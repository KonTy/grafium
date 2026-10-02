<script lang="ts">
  import type { RuntimeRecovery } from "../lib/knowledge";
  import { recoveryRole, recoveryState, recoverySummary } from "../lib/modelRecovery";
  import SettingsHelp from "./SettingsHelp.svelte";
  let { records, busy = false, compact = false, onRetry, onUseCpu }: {
    records: RuntimeRecovery[];
    busy?: boolean;
    compact?: boolean;
    onRetry: (key: string) => void | Promise<void>;
    onUseCpu?: (key: string) => void | Promise<void>;
  } = $props();
</script>

{#if records.length}
  <section aria-label="Model recovery" data-help-context="ai">
    {#if !compact}
      <div class="help-row">
        <h4>Model recovery</h4>
        <SettingsHelp title="Automatic model recovery">
          <p>Grafium tries faster mode once automatically after a model stops unexpectedly. If recovery fails, it remembers slower mode across restarts. No approval is needed.</p>
          <p>Try faster mode requests one more attempt, with memory checks still in place. Keep slower mode skips a pending retry. Neither action changes models or sends anything to a different provider.</p>
          <p>A retry is not proof of success. The recovery record remains until the worker exits cleanly. Slower mode uses CPU and still needs enough memory.</p>
        </SettingsHelp>
      </div>
    {/if}
    {#each records as recovery (recovery.key)}
      <div class="recovery">
        <p><strong>{recoveryRole(recovery.label)}</strong></p>
        <p role="status">{recoverySummary(recovery)}</p>
        <div class="actions">
          {#if recoveryState(recovery) === "cpu_only"}
            <button type="button" disabled={busy} aria-label={`${recoveryRole(recovery.label)}: Try faster mode`}
              onclick={() => onRetry(recovery.key)}>Try faster mode</button>
          {:else if recoveryState(recovery) === "retry_pending" && onUseCpu}
            <button type="button" disabled={busy} aria-label={`${recoveryRole(recovery.label)}: Keep slower mode`}
              onclick={() => onUseCpu?.(recovery.key)}>Keep slower mode</button>
          {/if}
        </div>
        <details>
          <summary>Technical details</summary>
          <p>{recovery.label}</p>
          <p>{recovery.reason}</p>
        </details>
      </div>
    {/each}
  </section>
{/if}

<style>
  section { width: 100%; }
  .help-row { display: flex; align-items: center; gap: 8px; }
  h4 { margin: 0; }
  .recovery { padding: 8px 0; }
  .recovery + .recovery { border-top: 1px solid var(--border); }
  p { margin: 0 0 6px; line-height: 1.5; overflow-wrap: anywhere; }
  .actions { display: flex; flex-wrap: wrap; gap: 6px; }
  details { color: var(--text-secondary); font-size: 12px; margin-top: 6px; }
  summary { cursor: pointer; }
  details p { margin-top: 6px; }
  button { font: inherit; background: var(--bg-primary); color: var(--text-primary); border: 1px solid var(--border); border-radius: 5px; padding: 5px 8px; cursor: pointer; }
  button:hover:not(:disabled) { background: var(--bg-hover); }
  button:disabled { opacity: .55; cursor: default; }
  button:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
