<script lang="ts">
  import { getGraphInfo, type Page } from "../lib/api";
  import BookReader from "./BookReader.svelte";
  import type { StudyProgress } from "../lib/studies";

  let { page, onStudyProgress }: { page: Page; onStudyProgress?: (progress: StudyProgress) => void } = $props();
  let graphPath = $state("");
  let error = $state("");
  let refresh = $state(0);
  $effect(() => {
    page.id;
    refresh;
    let disposed = false;
    graphPath = "";
    error = "";
    void getGraphInfo().then((graph) => {
      if (!disposed) graphPath = graph.path;
    }).catch((cause) => {
      if (!disposed) error = `Could not open the book's graph: ${String(cause)}`;
    });
    return () => { disposed = true; };
  });
</script>

{#if error}
  <div data-help-context="books">
    <p role="alert">{error}</p>
    <button onclick={() => refresh++}>Retry opening book</button>
  </div>
{:else if graphPath}
  <BookReader {page} {graphPath} {onStudyProgress} />
{:else}
  <p role="status">Opening book...</p>
{/if}
