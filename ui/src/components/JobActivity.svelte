<!--
  Ambient indicator for background AI work.

  Deliberately quiet: it only appears while something is actually running, sits
  out of the way, and never takes focus. The point is that a user who started
  an index can glance down and see it is still going, rather than wondering
  whether closing the settings panel killed it.
-->
<script lang="ts">
  import { jobs, cancelJob, type Job } from "../lib/jobs.svelte";

  const running = $derived(jobs.filter((j) => j.status === "running"));

  function percent(job: Job): number | null {
    return job.progress === null ? null : Math.round(job.progress * 100);
  }
</script>

{#if running.length > 0}
  <div class="job-activity" role="status" aria-live="polite">
    {#each running as job (job.id)}
      <div class="job">
        <div class="job-head">
          <span class="job-title">{job.title}</span>
          {#if job.cancellable}
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
        {/if}
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
      </div>
    {/each}
  </div>
{/if}

<style>
  .job-activity {
    position: fixed;
    bottom: 16px;
    left: 16px;
    z-index: 2900;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: 260px;
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
