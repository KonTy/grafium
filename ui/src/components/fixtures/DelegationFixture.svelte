<!--
  Test-only fixture that mirrors the shape `.query-block` used to have: a
  container wrapping controls whose handlers are ordinary Svelte `onclick`
  attributes, with an optional *native* click listener on the container that
  calls `stopPropagation()`.

  It exists so `BlockEditor.queryBlock.test.ts` can demonstrate the failure
  against the real Svelte runtime rather than against hand-rolled jsdom
  listeners.
-->
<script lang="ts">
  let { stopNatively = false, onInner }: { stopNatively?: boolean; onInner: () => void } = $props();

  let container = $state<HTMLDivElement>();

  $effect(() => {
    const el = container;
    if (!el || !stopNatively) return;
    const stop = (event: MouseEvent) => event.stopPropagation();
    el.addEventListener("click", stop);
    return () => el.removeEventListener("click", stop);
  });
</script>

<div class="container" bind:this={container}>
  <button class="inner" onclick={onInner}>run</button>
</div>
