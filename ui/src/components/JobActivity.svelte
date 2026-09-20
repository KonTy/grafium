<script lang="ts">
  import { jobs, cancelJob, clearFinishedJobs, isTerminal, type Job } from "../lib/jobs.svelte";

  let { toolbar = false }: { toolbar?: boolean } = $props();

  const running = $derived(jobs.filter((j) => j.status === "running"));
  const failed = $derived(jobs.filter((j) => j.status === "failed"));
  const recent = $derived([...jobs].reverse());
  const jobCount = $derived(running.length > 0 ? running.length : failed.length || jobs.length);
  let expanded = $state(false);
  let container: HTMLDivElement | null = $state(null);

  $effect(() => {
    if (!expanded) return;
    const closeOutside = (event: PointerEvent) => {
      if (!container?.contains(event.target as Node)) expanded = false;
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") expanded = false;
    };
    window.addEventListener("pointerdown", closeOutside);
    window.addEventListener("keydown", closeOnEscape);
    return () => {
      window.removeEventListener("pointerdown", closeOutside);
      window.removeEventListener("keydown", closeOnEscape);
    };
  });

  function percent(job: Job): number | null {
    return job.progress === null ? null : Math.round(job.progress * 100);
  }

  function statusLabel(job: Job): string {
    switch (job.status) {
      case "running":
        return "Running";
      case "succeeded":
        return "Done";
      case "failed":
        return "Failed";
      case "cancelled":
        return "Cancelled";
    }
  }

  function jobDetails(job: Job): string | null {
    return job.details ?? job.error ?? null;
  }
</script>

{#if jobs.length > 0}
  <div class:toolbar class="job-activity" bind:this={container}>
    <button
      type="button"
      class="job-toggle"
      class:running={running.length > 0}
      onclick={() => (expanded = !expanded)}
      aria-label={`${jobCount} ${jobCount === 1 ? "job" : "jobs"}`}
      aria-haspopup="dialog"
      aria-expanded={expanded}
    >
      <svg width="19" height="19" viewBox="0 0 24 24" fill="none" aria-hidden="true">
        <path d="M18 9a6 6 0 1 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9Z" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round" />
        <path d="M10 21h4" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" />
      </svg>
      <span class="job-badge">{jobCount}</span>
    </button>

    {#if expanded}
      <div class="job-panel" role="dialog" aria-label="Jobs">
        <div class="job-panel-head">
          <div>
            <strong>Jobs</strong>
            {#if running.length > 0}
              <span class="shimmer shimmer-endless">{running.length} running</span>
            {:else if failed.length > 0}
              <span>{failed.length} failed</span>
            {:else}
              <span>{jobs.length} recent</span>
            {/if}
          </div>
          <button class="job-clear" onclick={clearFinishedJobs} disabled={!jobs.some((j) => isTerminal(j.status))}>
            Clear finished
          </button>
        </div>
        {#each recent as job (job.id)}
          <div class="job" class:terminal={isTerminal(job.status)} class:failed={job.status === "failed"}>
            <div class="job-head">
              <span class="job-title">{job.title}</span>
              <span class="job-status {job.status}">{statusLabel(job)}</span>
              {#if job.cancellable && job.status === "running"}
                <button
                  class="job-cancel"
                  onclick={() => cancelJob(job.id)}
                  aria-label="Cancel {job.title}"
                  title="Cancel">×</button
                >
              {/if}
            </div>
            {#if job.message}
              <div class="job-message">{job.message}</div>
            {:else if job.error}
              <div class="job-message">{job.error}</div>
            {/if}
            {#if job.status === "running"}
              {#if percent(job) !== null}
                <div
                  class="job-bar"
                  role="progressbar"
                  aria-valuenow={percent(job)}
                  aria-valuemin="0"
                  aria-valuemax="100"
                >
                  <div class="job-bar-fill" style="width: {percent(job)}%"></div>
                </div>
              {:else}
                <div class="job-bar indeterminate"><div class="job-bar-fill"></div></div>
              {/if}
            {/if}
            {#if jobDetails(job)}
              <details class="job-details">
                <summary>Details</summary>
                <pre>{jobDetails(job)}</pre>
              </details>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
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
    border: 1px solid var(--bg);
    border-radius: 999px;
    background: var(--accent);
    color: var(--btn-primary-fg, var(--bg));
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

  .job-panel {
    position: absolute;
    top: calc(100% + 8px);
    right: 0;
    display: flex;
    width: min(380px, calc(100vw - 28px));
    max-height: min(520px, calc(100vh - 100px));
    flex-direction: column;
    gap: 8px;
    overflow: auto;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg);
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.35);
  }

  .job-panel-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    color: var(--text-muted);
    font-size: 12px;
  }

  .job-panel-head strong {
    display: block;
    color: var(--text);
    font-size: 13px;
  }

  .job-panel-head span {
    display: block;
    margin-top: 1px;
  }

  .job-clear {
    border: 1px solid var(--border);
    border-radius: 4px;
    background: transparent;
    color: var(--text-muted);
    font: inherit;
    font-size: 11px;
    padding: 2px 6px;
    cursor: pointer;
  }

  .job-clear:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .job {
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-secondary);
    color: var(--text);
    font-size: 12px;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.25);
  }

  .job.terminal {
    box-shadow: none;
  }

  .job.failed {
    border-left: 3px solid var(--error);
  }

  .job-head {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .job-title {
    flex: 1;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .job-status {
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: 11px;
    font-weight: 600;
  }

  .job-status.failed {
    color: var(--error);
  }

  .job-status.succeeded {
    color: var(--accent);
  }

  .job-cancel {
    flex-shrink: 0;
    background: none;
    border: none;
    color: var(--text-muted);
    font-size: 15px;
    line-height: 1;
    padding: 0 2px;
    cursor: pointer;
  }

  .job-cancel:hover {
    color: var(--text);
  }

  .job-message {
    margin-top: 2px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .job-details {
    margin-top: 6px;
    color: var(--text-muted);
    font-size: 11px;
  }

  .job-details summary {
    cursor: pointer;
  }

  .job-details pre {
    max-height: 220px;
    overflow: auto;
    margin: 6px 0 0;
    padding: 6px;
    border-radius: 4px;
    background: var(--bg-hover);
    color: var(--text);
    white-space: pre-wrap;
    word-break: break-word;
  }

  .job-bar {
    margin-top: 6px;
    height: 3px;
    border-radius: 2px;
    background: var(--bg-hover);
    overflow: hidden;
  }

  .job-bar-fill {
    height: 100%;
    background: var(--accent);
    transition: width 200ms ease-out;
  }

  .job-bar.indeterminate .job-bar-fill {
    width: 35%;
  }

  /* An indeterminate bar is the only thing here that animates, and only when
     the user hasn't asked us to stop moving things. */
  @media (prefers-reduced-motion: no-preference) {
    .job-bar.indeterminate .job-bar-fill {
      animation: job-slide 1.4s ease-in-out infinite;
    }
  }

  @keyframes job-slide {
    0% {
      transform: translateX(-100%);
    }
    100% {
      transform: translateX(285%);
    }
  }
</style>
