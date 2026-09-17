<script lang="ts">
  import { BUSY_STALL_MS, busyDisplay } from "../lib/busyState";

  interface Props {
    /** Whether the work is currently running. */
    active: boolean;
    /** The label shown while working, e.g. "Saving…". */
    text: string;
    /** Shown instead once the run overruns its deadline. */
    stalledText?: string;
    /**
     * How long this work may run before the shimmer is withdrawn. Pass 0 for
     * work that reports real progress and so needs no deadline.
     */
    stallAfterMs?: number;
  }

  let { active, text, stalledText = "", stallAfterMs = BUSY_STALL_MS }: Props = $props();

  let startedAt = $state<number | null>(null);
  let now = $state(Date.now());
  let reducedMotion = $state(false);

  $effect(() => {
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    reducedMotion = query.matches;
    const onChange = () => { reducedMotion = query.matches; };
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  });

  $effect(() => {
    if (!active) { startedAt = null; return; }
    startedAt = Date.now();
    now = Date.now();
    // Ticking only matters up to the deadline: after that the display is
    // settled and a running timer would be pure waste.
    if (stallAfterMs <= 0) return;
    const clock = setInterval(() => { now = Date.now(); }, 1000);
    return () => clearInterval(clock);
  });

  const display = $derived(busyDisplay(startedAt, now, reducedMotion, stallAfterMs));
</script>

{#if active}
  <span class="busy-label" class:stalled={display.stalled} role="status" aria-live="polite">
    <span class:shimmer={display.shimmer}>{display.stalled && stalledText ? stalledText : text}</span>
  </span>
{/if}

<style>
  .busy-label { overflow-wrap: anywhere; }
  .busy-label.stalled { color: var(--warning-color, #c9873a); }
</style>
