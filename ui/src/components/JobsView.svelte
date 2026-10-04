<script lang="ts">
  import { untrack } from "svelte";
  import {
    jobs,
    jobsSeen,
    cancelJob,
    clearFinishedJobs,
    groupJobs,
    isTerminal,
    latestFinish,
    markJobsSeen,
    unseenResults,
    type Job,
    type JobLink,
  } from "../lib/jobs.svelte";

  interface Props {
    onOpenPage?: (link: JobLink) => void;
  }

  let { onOpenPage }: Props = $props();

  // Read before this visit marks everything as seen, so the results the bell
  // announced stay marked New for as long as the page is open.
  const seenBefore = jobsSeen.until;
  const newIds = $derived(new Set(unseenResults(jobs, seenBefore).map((job) => job.id)));

  const groups = $derived(groupJobs(jobs));
  const running = $derived(groups.running);
  const failed = $derived(groups.failed);
  const completed = $derived(groups.completed);
  const sections = $derived([
    { id: "running", title: "Running", jobs: running },
    { id: "failed", title: "Failed", jobs: failed },
    { id: "completed", title: "Completed", jobs: completed },
    { id: "cancelled", title: "Cancelled", jobs: groups.cancelled },
  ]);

  $effect(() => {
    const finished = latestFinish(jobs);
    untrack(() => markJobsSeen(finished));
  });

  function statusLabel(job: Job): string {
    switch (job.status) {
      case "running":
        return "Running";
      case "succeeded":
        return "Completed";
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

  function timing(job: Job): string {
    const times = [];
    if (job.started_at) times.push(`Started ${formatTimestamp(job.started_at)}`);
    if (job.finished_at) times.push(`Finished ${formatTimestamp(job.finished_at)}`);
    return times.join(" · ");
  }
</script>

<div class="jobs-view">
  <div class="jobs-header">
    <div>
      <h1>Jobs</h1>
      <p>Background imports, indexing work, progress, and failures. The last 100 finished jobs stay here across restarts, on this device only.</p>
    </div>
    <button
      class="clear-btn"
      onclick={clearFinishedJobs}
      disabled={!jobs.some((job) => isTerminal(job.status))}
      title="Remove every finished job from the history. Running jobs stay."
    >
      Clear history
    </button>
  </div>

  <div class="summary-grid">
    <div class="summary-card">
      <span class="summary-count">{jobs.length}</span>
      <span class="summary-label">Total</span>
    </div>
    <div class="summary-card">
      <span class="summary-count">{running.length}</span>
      <span
        class="summary-label"
        class:shimmer={running.length > 0}
        class:shimmer-endless={running.length > 0}
      >Running</span>
    </div>
    <div class="summary-card">
      <span class="summary-count">{completed.length}</span>
      <span class="summary-label">Completed</span>
    </div>
    <div class="summary-card" class:has-failures={failed.length > 0}>
      <span class="summary-count">{failed.length}</span>
      <span class="summary-label">Failed</span>
    </div>
  </div>

  {#if jobs.length === 0}
    <div class="empty-state">
      <h2>No jobs yet</h2>
      <p>Start a book, media, or index job and its status will appear here.</p>
    </div>
  {:else}
    {#each sections as section (section.id)}
      {#if section.jobs.length > 0}
        <section class="job-section" aria-labelledby={`jobs-${section.id}`}>
          <h2 id={`jobs-${section.id}`}>{section.title}</h2>
          <div class="job-list">
            {#each section.jobs as job (job.id)}
              {@render jobCard(job)}
            {/each}
          </div>
        </section>
      {/if}
    {/each}
  {/if}
</div>

{#snippet jobCard(job: Job)}
  <article
    class="job-card"
    class:running={job.status === "running"}
    class:failed={job.status === "failed"}
  >
    <header class="job-head">
      <div>
        <h3>{job.title}</h3>
        <p>{timing(job)}</p>
      </div>
      <div class="job-labels">
        {#if newIds.has(job.id)}
          <span class="new-label">New</span>
        {/if}
        <span class="status {job.status}">
          {#if job.status === "running"}
            <span class="status-pulse" aria-hidden="true"></span>
          {/if}
          {statusLabel(job)}
        </span>
      </div>
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
{/snippet}

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

  .job-section + .job-section {
    margin-top: 24px;
  }

  .job-section h2 {
    margin: 0 0 10px;
    color: var(--text-muted);
    font-size: 12px;
    font-weight: 650;
    text-transform: uppercase;
    letter-spacing: 0.05em;
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

  .job-head h3 {
    margin: 0 0 4px;
    font-size: 16px;
  }

  .job-labels {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .new-label {
    border: 1px solid color-mix(in srgb, var(--accent) 55%, var(--border));
    border-radius: 999px;
    padding: 2px 7px;
    color: var(--accent);
    font-size: 12px;
    font-weight: 650;
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

  @media (max-width: 640px) {
    .jobs-view {
      padding: 20px 14px;
    }
  }
</style>
