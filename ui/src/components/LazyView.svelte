<script lang="ts" generics="T">
  import type { Snippet } from "svelte";

  let { load, name, children }: {
    load: () => Promise<{ default: T }>;
    name: string;
    children: Snippet<[T]>;
  } = $props();

  const pending = $derived(load());
</script>

{#await pending}
  <div class="lazy-view-status" role="status">Loading {name}...</div>
{:then module}
  {@render children(module.default)}
{:catch error}
  <div class="lazy-view-status" role="alert">
    <p>Could not load {name}: {error instanceof Error ? error.message : String(error)}</p>
    <p>Save any active edits, then reload the app to retry.</p>
    <button onclick={() => window.location.reload()}>Reload app</button>
  </div>
{/await}

<style>
  .lazy-view-status {
    padding: 24px;
    color: var(--text-secondary);
  }
  button {
    margin-top: 12px;
  }
</style>
