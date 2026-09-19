<!--
  Test-only fixture mirroring BlockEditor's rendered-content nesting: an
  ancestor (`.block-content`, whose `handleClick` opens the editor) carrying a
  *native* click listener, wrapping the rendered markdown container whose own
  handler wants to claim clicks on checkboxes, links and table headers.

  `renderedNative` switches the inner container between the two wirings so
  `BlockEditor.renderedClick.test.ts` can show what each one does to the
  ancestor.
-->
<script lang="ts">
  let {
    renderedNative = false,
    onAncestor,
    onRendered,
  }: { renderedNative?: boolean; onAncestor: () => void; onRendered: () => void } = $props();

  let ancestor = $state<HTMLDivElement>();
  let rendered = $state<HTMLDivElement>();

  // Stands in for `.block-content`'s pre-existing native `handleClick`.
  $effect(() => {
    const el = ancestor;
    if (!el) return;
    const handler = () => onAncestor();
    el.addEventListener("click", handler);
    return () => el.removeEventListener("click", handler);
  });

  function claim(event: MouseEvent) {
    event.stopPropagation();
    event.preventDefault();
    onRendered();
  }

  $effect(() => {
    const el = rendered;
    if (!el || !renderedNative) return;
    el.addEventListener("click", claim);
    return () => el.removeEventListener("click", claim);
  });
</script>

<div class="ancestor" bind:this={ancestor}>
  {#if renderedNative}
    <div class="rendered" bind:this={rendered}>
      <input class="task-checkbox" type="checkbox" />
    </div>
  {:else}
    <!-- Non-interactive container: the checkbox inside is the real control.
         Same reasoning as the app's other markup-handler containers. -->
    <div class="rendered" role="presentation" onclick={claim}>
      <input class="task-checkbox" type="checkbox" />
    </div>
  {/if}
</div>
