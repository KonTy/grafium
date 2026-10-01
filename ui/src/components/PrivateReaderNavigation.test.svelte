<script lang="ts">
  import PrivateReaderToolbar from "./PrivateReaderToolbar.svelte";
  let page = $state("Studies");
  let graph = $state("First graph");
</script>

<PrivateReaderToolbar onOpen={() => page = "Studies"} />
<nav aria-label="Test navigation">
  <button onclick={() => page = "Journal"}>Go to Journal</button>
  <button onclick={() => graph = "Other graph"}>Switch graph</button>
</nav>
{#key graph}
  <main data-graph={graph}><h1>{page}</h1></main>
{/key}
