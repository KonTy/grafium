<script lang="ts">
  import {
    jobs,
    cancelJob,
    clearFinishedJobs,
    isTerminal,
    type Job,
    type JobLink,
  } from "../lib/jobs.svelte";

  interface Props {
    onOpenPage?: (link: JobLink) => void;
  }

  let { onOpenPage }: Props = $props();

  const recent = $derived([...jobs].reverse());
  const running = $derived(jobs.filter((job) => job.status === "running"));
  const failed = $derived(jobs.filter((job) => job.status === "failed"));

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

  function percent(job: Job): number | null {
    return job.progress === null ? null : Math.round(job.progress * 100);
  }

  function jobDetails(job: Job): string | null {
    return job.details ?? job.error ?? null;
  }

  function formatTimestamp(ms: number | null): string {
    if (!ms) return "";
    return new Date(ms).toLocaleString();
  }
</script>

<div class="jobs-view">
  <div class="jobs-header">
    <div>
      <h1>Jobs</h1>
      <p>Background imports, indexing work, progress, failures, and job history from this app session.</p>
    </div>
    <button
      class="clear-btn"
      onclick={clearFinishedJobs}
      disabled={!jobs.some((job) => isTerminal(job.status))}
    >
      Clear finished
    </button>
  </div>

  <div class="summary-grid">
    <div class="summary-card">
      <span class="summary-count">{jobs.length}</span>
      <span class="summary-label">Total</span>
    </div>
    <div class="summary-card">
      <span class="summary-count">{running.length}</span>
      <span class="summary-label shimmer shimmer-endless">Running</span>
    </div>
    <div class="summary-card" class:has-failures={failed.length > 0}>
      <span class="summary-count">{failed.length}</span>
      <span class="summary-label">Failed</span>
    </div>
  </div>

  {#if recent.length === 0}
    <div class="empty-state">
      <h2>No jobs yet</h2>
      <p>Start a book, media, or index job and its status will appear here.</p>
    </div>
  {:else}
    <div class="job-list">
      {#each recent as job (job.id)}
        <article
          class="job-card"
          class:running={job.status === "running"}
          class:failed={job.status === "failed"}
        >
          <header class="job-head">
            <div>
              <h2>{job.title}</h2>
              <p>{formatTimestamp(job.started_at)}{job.finished_at ? ` - ${formatTimestamp(job.finished_at)}` : ""}</p>
            </div>
            <span class="status {job.status}">
              {#if job.status === "running"}
                <span class="status-pulse" aria-hidden="true"></span>
              {/if}
              {statusLabel(job)}
            </span>
          </header>

          {#if job.message}
            <p class="job-message">{job.message}</p>
          {:else if job.error}
            <p class="job-message error">{job.error}</p>
          {/if}

          {#if job.status === "running"}
            <div
              class="progress"
              class:indeterminate={percent(job) === null}
              role="progressbar"
              aria-valuenow={percent(job) ?? undefined}
              aria-valuemin="0"
              aria-valuemax="100"
            >
              <div
                class="progress-fill"
                style={percent(job) === null ? undefined : `width: ${percent(job)}%;`}
              ></div>
            </div>
          {/if}

          <div class="job-actions">
            {#if job.link && onOpenPage}
              <button class="secondary-btn" onclick={() => onOpenPage?.(job.link!)}>
                Open {job.link.label}
              </button>
            {/if}
            {#if job.cancellable && job.status === "running"}
              <button class="secondary-btn danger" onclick={() => cancelJob(job.id)}>Cancel</button>
            {/if}
          </div>

          {#if jobDetails(job)}
            <details class="details" open={job.status === "failed"}>
              <summary>Details</summary>
              <pre>{jobDetails(job)}</pre>
            </details>
          {/if}
        </article>
      {/each}
    </div>
  {/if}
</div>

<style>
  .jobs-view {
    max-width: 980px;
    margin: 0 auto;
    padding: 32px;
    color: var(--text);
  }

  .jobs-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 20px;
  }

  h1 {
    margin: 0 0 6px;
    font-size: 28px;
    font-weight: 650;
  }

  .jobs-header p,
  .empty-state p,
  .job-head p {
    margin: 0;
    color: var(--text-muted);
  }

  .clear-btn,
  .secondary-btn {
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-secondary);
    color: var(--text);
    font: inherit;
    font-size: 13px;
    padding: 7px 10px;
    cursor: pointer;
  }

  .clear-btn:hover:not(:disabled),
  .secondary-btn:hover {
    background: var(--bg-hover);
    border-color: var(--accent);
  }

  .clear-btn:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .summary-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
    gap: 12px;
    margin-bottom: 20px;
  }

  .summary-card {
    padding: 14px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg-secondary);
  }

  .summary-card.has-failures {
    border-left: 3px solid var(--error);
  }

  .summary-count {
    display: block;
    font-size: 24px;
    font-weight: 700;
  }

  .summary-label {
    color: var(--text-muted);
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .empty-state {
    padding: 28px;
    border: 1px dashed var(--border);
    border-radius: 10px;
    background: var(--bg-secondary);
    text-align: center;
  }

  .empty-state h2 {
    margin: 0 0 6px;
    font-size: 18px;
  }

  .job-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .job-card {
    padding: 14px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg-secondary);
  }

  .job-card.running {
    border-color: color-mix(in srgb, var(--accent) 45%, var(--border));
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--accent) 14%, transparent);
  }

  .job-card.failed {
    border-left: 3px solid var(--error);
  }

  .job-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }

  .job-head h2 {
    margin: 0 0 4px;
    font-size: 16px;
  }

  .status {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
    border-radius: 999px;
    padding: 3px 8px;
    background: var(--bg-hover);
    color: var(--text-muted);
    font-size: 12px;
    font-weight: 650;
  }

  .status.running {
    background: color-mix(in srgb, var(--accent) 12%, var(--bg-hover));
    color: var(--accent);
  }

  .status-pulse {
    width: 7px;
    height: 7px;
    border-radius: 999px;
    background: currentColor;
  }

  .status.failed {
    color: var(--error);
  }

  .status.succeeded {
    color: var(--accent);
  }

  .job-message {
    margin: 12px 0 0;
    color: var(--text);
  }

  .job-message.error {
    color: var(--error);
  }

  .progress {
    height: 4px;
    margin-top: 12px;
    overflow: hidden;
    border-radius: 999px;
    background: var(--bg-hover);
  }

  .progress-fill {
    width: 35%;
    height: 100%;
    background: var(--accent);
    transition: width 200ms ease-out;
  }

  .progress.indeterminate .progress-fill {
    width: 35%;
  }

  @media (prefers-reduced-motion: no-preference) {
    .status.running .status-pulse {
      animation: job-status-pulse 1s ease-in-out infinite;
    }

    .progress.indeterminate .progress-fill {
      animation: job-progress-slide 1.35s ease-in-out infinite;
    }
  }

  @keyframes job-status-pulse {
    0%,
    100% {
      opacity: 0.35;
      transform: scale(0.85);
    }
    50% {
      opacity: 1;
      transform: scale(1.15);
    }
  }

  @keyframes job-progress-slide {
    0% {
      transform: translateX(-110%);
    }
    100% {
      transform: translateX(285%);
    }
  }

  .job-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 12px;
  }

  .secondary-btn.danger:hover {
    border-color: var(--error);
  }

  .details {
    margin-top: 12px;
    color: var(--text-muted);
  }

  .details summary {
    cursor: pointer;
    font-weight: 600;
  }

  .details pre {
    max-height: 420px;
    overflow: auto;
    margin: 8px 0 0;
    padding: 10px;
    border-radius: 6px;
    background: var(--bg);
    color: var(--text);
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
