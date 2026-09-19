<script lang="ts">
  import { autofocus } from "../../lib/autofocus";

  let open = $state(false);
</script>

<button class="open" onclick={() => (open = true)}>Open</button>

{#if open}
  <div class="panel" tabindex="-1" use:autofocus>
    <button class="close" onclick={() => (open = false)}>Close</button>
  </div>
{/if}
