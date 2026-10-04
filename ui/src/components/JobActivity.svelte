<script lang="ts">
  import { jobs, jobsSeen, jobIndicator } from "../lib/jobs.svelte";

  let { toolbar = false, onOpen }: { toolbar?: boolean; onOpen: () => void } = $props();

  const indicator = $derived(jobIndicator(jobs, jobsSeen.until));
  const label = $derived(indicator.summary ? `Jobs: ${indicator.summary}` : "Jobs");
</script>

{#if jobs.length > 0}
  <div class:toolbar class="job-activity">
    <button
      type="button"
      class="job-toggle"
      class:running={indicator.running}
      data-tauri-drag-region="false"
      onclick={onOpen}
      title={label}
      aria-label={label}
    >
      <svg width="19" height="19" viewBox="0 0 24 24" fill="none" aria-hidden="true">
        <path d="M18 9a6 6 0 1 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9Z" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round" />
        <path d="M10 21h4" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" />
      </svg>
      {#if indicator.count > 0}
        <span class="job-badge" aria-hidden="true">{indicator.count > 99 ? "99+" : indicator.count}</span>
      {/if}
    </button>
  </div>
{/if}

<style>
  .job-activity {
    position: fixed;
    top: calc(44px + env(safe-area-inset-top, 0px));
    right: 14px;
    z-index: 2900;
  }

  .job-activity.toolbar {
    position: relative;
    top: auto;
    right: auto;
    z-index: auto;
    flex-shrink: 0;
  }

  .job-toggle {
    position: relative;
    display: grid;
    place-items: center;
    width: 36px;
    height: 36px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--bg-secondary);
    color: var(--text);
    font: inherit;
    cursor: pointer;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.25);
  }

  /* The bell is drawn round, but its whole square title-bar slot takes the
     click, like the buttons beside it. */
  .job-activity.toolbar .job-toggle::before {
    content: "";
    position: absolute;
    inset: -1px;
  }

  .job-toggle:hover {
    background: var(--bg-hover);
  }

  .job-toggle.running {
    border-color: color-mix(in srgb, var(--accent) 55%, var(--border));
  }

  .job-toggle:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  .job-badge {
    position: absolute;
    top: -6px;
    right: -6px;
    display: grid;
    place-items: center;
    min-width: 18px;
    height: 18px;
    padding: 0 4px;
    border: 1px solid var(--bg-primary);
    border-radius: 999px;
    background: var(--accent);
    color: var(--btn-primary-fg, var(--bg-primary));
    font-size: 10px;
    font-weight: 700;
    line-height: 1;
  }

  .job-activity.toolbar .job-badge {
    top: 1px;
    right: 1px;
  }

  @media (prefers-reduced-motion: no-preference) {
    .job-toggle.running::after {
      content: "";
      position: absolute;
      inset: -5px;
      border: 1px solid var(--accent);
      border-radius: inherit;
      opacity: 0;
      animation: job-toggle-ring 1.6s ease-out infinite;
      pointer-events: none;
    }
  }

  @keyframes job-toggle-ring {
    0% {
      opacity: 0.7;
      transform: scale(0.88);
    }
    100% {
      opacity: 0;
      transform: scale(1.18);
    }
  }
</style>
