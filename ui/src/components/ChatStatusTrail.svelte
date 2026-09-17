<script lang="ts">
  import type { TrailDisplay } from "../lib/chatStatus";

  interface Props {
    trail: TrailDisplay;
    /** Progress detail for the live step when the backend reports one. */
    note?: string;
    /** Honest terminal text (stall, error) shown under the rows. */
    notice?: string;
    /** Overrides the ticking total, so throughput (tok/s) isn't lost when the
     *  status line moves out of the composer. */
    meta?: string;
    /** Collapse finished trails so old answers stay readable. */
    collapsed?: boolean;
  }

  let { trail, note = "", notice = "", meta = "", collapsed = false }: Props = $props();
</script>

{#if trail.any}
  {#if collapsed && !trail.running}
    <details class="status-trail-collapsed">
      <summary>Worked for {trail.finishedIn} · {trail.rows.length}
        {trail.rows.length === 1 ? "step" : "steps"}</summary>
      <ol class="status-trail">
        {#each trail.rows as row (row.key)}
          <li class="trail-row done">
            <span class="trail-mark" aria-hidden="true"></span>
            <span class="trail-label">{row.label}</span>
            {#if row.meta}<span class="trail-meta">{row.meta}</span>{/if}
          </li>
        {/each}
      </ol>
    </details>
  {:else}
    <!-- Polite, not assertive: a phase change should not interrupt a screen
         reader mid-sentence. The ticking total is aria-hidden so it is never
         re-announced every second. -->
    <ol class="status-trail" role="status" aria-live="polite">
      {#each trail.rows as row (row.key)}
        <li class="trail-row {row.state}">
          <span class="trail-mark" aria-hidden="true"></span>
          <span class="trail-label" class:shimmer={row.shimmer}>{row.label}</span>
          {#if row.state !== "done" && note}
            <span class="trail-note">· {note}</span>
          {:else if row.note}
            <span class="trail-note">· {row.note}</span>
          {/if}
          {#if row.meta}<span class="trail-meta">{row.meta}</span>{/if}
        </li>
      {/each}
      {#if trail.running}
        <li class="trail-row elapsed">
          <span class="trail-mark clock" aria-hidden="true"></span>
          <span class="trail-meta" aria-hidden="true">{meta || trail.elapsed}</span>
        </li>
      {/if}
    </ol>
    {#if notice}<p class="trail-notice">{notice}</p>{/if}
  {/if}
{/if}

<style>
  .status-trail {
    list-style: none;
    margin: 4px 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 12px;
    line-height: 1.5;
  }

  .trail-row {
    display: flex;
    align-items: baseline;
    gap: 7px;
    min-width: 0;
    color: var(--text-secondary);
  }

  .trail-mark {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-secondary);
    opacity: 0.45;
    transform: translateY(-1px);
  }

  .trail-row.active .trail-mark { opacity: 1; background: var(--accent); }
  .trail-row.stalled .trail-mark { opacity: 1; background: var(--warning-color, #c9873a); }
  .trail-mark.clock { border-radius: 1px; opacity: 0.35; }

  .trail-label { overflow-wrap: anywhere; }
  .trail-row.active .trail-label { color: var(--text-primary); }

  .trail-note, .trail-meta {
    color: var(--text-secondary);
    opacity: 0.75;
    overflow-wrap: anywhere;
  }

  .trail-meta { font-variant-numeric: tabular-nums; }

  /* The sweep travels across the glyphs themselves, so the row reads as "this
     step is running" without a spinner competing for attention. It is applied
     only to the single active row, and only when `shimmer` is true — which the
     pure status layer already turns off for reduced motion and stalled runs. */
  .shimmer {
    background: linear-gradient(
      100deg,
      var(--text-secondary) 20%,
      var(--text-primary) 42%,
      var(--text-primary) 52%,
      var(--text-secondary) 74%
    );
    background-size: 220% 100%;
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
    animation: trail-shimmer 2.1s linear infinite;
  }

  @keyframes trail-shimmer {
    from { background-position: 180% 0; }
    to { background-position: -80% 0; }
  }

  .status-trail-collapsed { margin: 4px 0; }

  .status-trail-collapsed > summary {
    cursor: pointer;
    font-size: 12px;
    color: var(--text-secondary);
    opacity: 0.8;
    list-style-position: outside;
  }

  .status-trail-collapsed > summary:hover { opacity: 1; }
  .status-trail-collapsed .status-trail { margin-left: 4px; }

  .trail-notice {
    margin: 2px 0 0;
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-secondary);
    overflow-wrap: anywhere;
  }

  /* Belt and braces: the shimmer is already gated in `statusTrail`, but a user
     who prefers reduced motion must never see it even if a caller forgets. */
  @media (prefers-reduced-motion: reduce) {
    .shimmer { animation: none; background: none; color: var(--text-primary); -webkit-text-fill-color: currentColor; }
  }
</style>
