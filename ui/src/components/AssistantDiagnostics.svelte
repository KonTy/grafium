<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import { aiIndexStatus, aiIndexAllPages, aiRetryLlmOnGpu, type IndexStatus } from "../lib/knowledge";
  let { active = true, running = false, onOpenSettings = () => {} }: {
    active?: boolean; running?: boolean; onOpenSettings?: () => void;
  } = $props();
  let index = $state<IndexStatus | null>(null);
  let error = $state("");
  let notice = $state("");
  let indexing = $state(false);
  let retrying = $state(false);
  async function refresh() {
    try { index = await aiIndexStatus(); error = ""; }
    catch (cause) { error = `Could not read index status: ${String(cause)}`; }
  }
  $effect(() => {
    if (!active) return;
    void refresh();
    const subscription = listen("ai-index-updated", () => { void refresh(); });
    return () => { void subscription.then((unlisten) => unlisten()).catch(() => {}); };
  });
  async function buildIndex() {
    if (indexing) return;
    indexing = true; error = ""; notice = "";
    try {
      const result = await aiIndexAllPages();
      notice = `Indexed ${result.pages_processed} pages; ${result.pages_failed} failed.`;
      await refresh();
    } catch (cause) { error = String(cause); }
    finally { indexing = false; }
  }
  async function retryGpu() {
    if (retrying || running) return;
    retrying = true; error = "";
    try { await aiRetryLlmOnGpu(); await refresh(); }
    catch (cause) { error = String(cause); }
    finally { retrying = false; }
  }
</script>

<details class="diagnostics assistant-disclosure">
  <summary>Model &amp; index status{index?.accelerator?.gpu_supported && !index.accelerator.on_gpu ? " · Running on CPU" : ""}</summary>
  <div class="diagnostic-content assistant-disclosure-body">
    {#if index}
      <p>{index.indexed_chunks} indexed chunks · {index.total_blocks} blocks · {index.pending_pages} pages pending</p>
      {#if !index.indexed_chunks}<p>No semantic index yet. Page and block questions still use exact source retrieval.</p>{/if}
      {#if !index.embedder_ready}<p>Configure an embedding model to index notes and use semantic search.</p>{/if}
      <button disabled={indexing || !index.embedder_ready} onclick={buildIndex}>{indexing ? "Indexing…" : "Index now"}</button>
      {#if index.accelerator?.gpu_supported && !index.accelerator.on_gpu}
        <p>Running on CPU despite GPU support. Retrying reloads the same model; it does not select another one.</p>
        <button disabled={retrying || running} onclick={retryGpu}>{retrying ? "Retrying…" : "Retry on GPU"}</button>
      {/if}
    {/if}
    {#if error}<p role="alert">{error}</p><button onclick={refresh}>Retry status</button>{/if}
    {#if notice}<p role="status">{notice}</p>{/if}
    <button onclick={onOpenSettings}>Configure provider</button>
  </div>
</details>

<style>
  .diagnostics { font-size: 12px; color: var(--text-secondary); }
  .diagnostic-content { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
  p { flex-basis: 100%; margin: 0; line-height: 1.5; overflow-wrap: anywhere; }
  button { font: inherit; background: var(--bg-primary); color: var(--text-primary); border: 1px solid var(--border); border-radius: 5px; padding: 5px 8px; cursor: pointer; }
  button:hover:not(:disabled) { background: var(--bg-hover); }
  button:disabled { opacity: .55; cursor: default; }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
