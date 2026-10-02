<script lang="ts">
  import { untrack } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { aiIndexStatus, aiIndexAllPages, aiRetryLlmOnGpu, type IndexStatus } from "../lib/knowledge";
  let {
    active = true, running = false, onOpenSettings = () => {},
    provider, connected, checking = false, noNotesExcludesHistory = false, onNotice,
  }: {
    active?: boolean;
    running?: boolean;
    onOpenSettings?: () => void;
    provider?: { label: string; detail: string };
    connected?: boolean;
    checking?: boolean;
    noNotesExcludesHistory?: boolean;
    onNotice?: (notice: { text: string; error: boolean } | null) => void;
  } = $props();
  const id = $props.id();
  let index = $state<IndexStatus | null>(null);
  let error = $state("");
  let statusError = $state("");
  let notice = $state("");
  let indexFailed = $state(false);
  let indexing = $state(false);
  let retrying = $state(false);
  let loading = $state(false);
  let opened = $state(false);
  let trigger: HTMLButtonElement;
  let menu: HTMLDivElement;
  let closeButton: HTMLButtonElement;
  let generation = 0;
  let statusRequest = 0;

  const cpuFallback = $derived(!!index?.accelerator?.gpu_supported && !index.accelerator.on_gpu);
  const needsRecovery = $derived(!!index?.runtime_recovery?.length);
  const disconnected = $derived(!checking && (connected === false || (connected === undefined && index?.llm_ready === false)));
  const warning = $derived(needsRecovery || cpuFallback || !!index?.runtime_warnings?.length || indexFailed || disconnected);
  const statusKind = $derived(error || statusError ? "error" : warning ? "warning"
    : running || indexing || retrying ? "working" : checking || loading || !index ? "checking" : "ready");
  const statusLabel = $derived(statusKind === "error" ? "Model attention needed: error"
    : needsRecovery ? "Model attention needed: GPU recovery approval required"
    : cpuFallback ? "Model attention needed: running on CPU"
    : disconnected ? "Model attention needed: configure provider"
    : statusKind === "warning" ? "Model attention needed: warning"
    : statusKind === "working" ? "Model working"
    : statusKind === "checking" ? "Checking model status" : "Model ready");
  const attention = $derived.by(() => {
    if (!active) return null;
    if (error || statusError) return { text: "Model attention needed — open status for the error and retry options.", error: true };
    if (needsRecovery) return { text: "Model attention needed — review GPU recovery approval in Settings.", error: false };
    if (cpuFallback) return { text: "Model attention needed — running on CPU; open status for safe GPU retry.", error: false };
    if (disconnected) return { text: "Model attention needed — configure a provider.", error: false };
    if (warning) return { text: "Model attention needed — open status to review warnings.", error: false };
    return null;
  });
  $effect(() => {
    const value = attention;
    const notify = onNotice;
    untrack(() => notify?.(value));
  });

  function current(token: number) { return active && token === generation; }
  async function refresh() {
    if (!active) return;
    const token = generation;
    const request = ++statusRequest;
    loading = true;
    try {
      const result = await aiIndexStatus();
      if (!current(token) || request !== statusRequest) return;
      index = result; statusError = "";
    } catch (cause) {
      if (current(token) && request === statusRequest) statusError = `Could not read index status: ${String(cause)}`;
    } finally {
      if (current(token) && request === statusRequest) loading = false;
    }
  }
  $effect(() => {
    const enabled = active;
    const token = ++generation;
    untrack(() => {
      index = null; error = ""; statusError = ""; notice = "";
      indexFailed = false; indexing = false; retrying = false; loading = false;
      if (!enabled) closeMenu(false);
    });
    if (!enabled) return;
    const subscription = listen("ai-index-updated", () => {
      if (current(token) && !running) void refresh();
    }).catch((cause) => {
      console.error("Could not subscribe to model index updates", cause);
      if (current(token)) statusError = `Could not subscribe to model index updates: ${String(cause)}`;
      return undefined;
    });
    return () => {
      ++generation;
      closeMenu(false);
      untrack(() => onNotice?.(null));
      void subscription.then((unlisten) => unlisten?.())
        .catch((cause) => console.error("Could not unsubscribe from model index updates", cause));
    };
  });
  $effect(() => {
    if (active && !running) untrack(() => { void refresh(); });
  });

  async function buildIndex() {
    if (!active || indexing || !index?.embedder_ready) return;
    const token = generation;
    indexing = true; error = ""; notice = ""; indexFailed = false;
    try {
      const result = await aiIndexAllPages();
      if (!current(token)) return;
      notice = `Indexed ${result.pages_processed} pages; ${result.pages_failed} failed.`;
      indexFailed = result.pages_failed > 0;
      await refresh();
    } catch (cause) { if (current(token)) error = `Could not index notes: ${String(cause)}`; }
    finally { if (current(token)) indexing = false; }
  }
  async function retryGpu() {
    if (!active || retrying || running || needsRecovery) return;
    const token = generation;
    retrying = true; error = "";
    try {
      await aiRetryLlmOnGpu();
      if (current(token)) await refresh();
    } catch (cause) { if (current(token)) error = `Could not retry on GPU: ${String(cause)}`; }
    finally { if (current(token)) retrying = false; }
  }

  function positionMenu() {
    if (!menu || !trigger || !opened) return;
    const viewport = window.visualViewport;
    const left = viewport?.offsetLeft ?? 0;
    const top = viewport?.offsetTop ?? 0;
    const width = viewport?.width ?? window.innerWidth;
    const height = viewport?.height ?? window.innerHeight;
    menu.style.maxWidth = `${Math.max(0, width - 16)}px`;
    menu.style.maxHeight = `${Math.max(0, height - 16)}px`;
    const anchor = trigger.getBoundingClientRect();
    const box = menu.getBoundingClientRect();
    const x = Math.max(left + 8, Math.min(anchor.right - box.width, left + width - box.width - 8));
    const above = anchor.top - box.height - 8;
    const y = Math.max(top + 8, Math.min(above >= top + 8 ? above : anchor.bottom + 8, top + height - box.height - 8));
    menu.style.left = `${x}px`;
    menu.style.top = `${y}px`;
  }
  export function openMenu() {
    if (!active || opened) return;
    try {
      menu.showPopover();
      opened = true;
      positionMenu();
      closeButton.focus({ preventScroll: true });
    } catch (cause) {
      error = `Could not open model status: ${String(cause)}`;
    }
  }
  function closeMenu(restoreFocus = true) {
    if (!opened) return;
    menu?.hidePopover();
    opened = false;
    if (restoreFocus && active) trigger?.focus({ preventScroll: true });
  }
  function menuToggle(event: Event) {
    opened = (event as ToggleEvent).newState === "open";
  }
  function openSettings() {
    closeMenu();
    onOpenSettings();
  }
  $effect(() => {
    if (!opened) return;
    const contextualHelp = (event: KeyboardEvent) => {
      if (event.key === "F1") closeMenu();
    };
    const observer = new ResizeObserver(positionMenu);
    observer.observe(menu);
    window.addEventListener("keydown", contextualHelp, true);
    window.addEventListener("resize", positionMenu);
    window.addEventListener("scroll", positionMenu, true);
    window.visualViewport?.addEventListener("resize", positionMenu);
    window.visualViewport?.addEventListener("scroll", positionMenu);
    return () => {
      observer.disconnect();
      window.removeEventListener("keydown", contextualHelp, true);
      window.removeEventListener("resize", positionMenu);
      window.removeEventListener("scroll", positionMenu, true);
      window.visualViewport?.removeEventListener("resize", positionMenu);
      window.visualViewport?.removeEventListener("scroll", positionMenu);
    };
  });
</script>

<button bind:this={trigger} type="button" class="model-status-trigger" data-status={statusKind}
  data-help-context="chat"
  aria-label={`Model & index status: ${statusLabel}`} title={`Model & index status: ${statusLabel}`}
  aria-haspopup="dialog" aria-expanded={opened} aria-controls={`${id}-model-status`} disabled={!active}
  onclick={() => opened ? closeMenu() : openMenu()}>
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
    stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" data-status-icon={statusKind}>
    {#if statusKind === "ready"}
      <circle cx="12" cy="12" r="9" /><path d="m8 12 3 3 5-6" />
    {:else if statusKind === "working"}
      <path d="M8 3h8M8 21h8M8 3v4l8 10v4M16 3v4L8 17v4" />
    {:else if statusKind === "warning"}
      <path d="m12 3 10 18H2Z M12 9v5 M12 17h.01" />
    {:else if statusKind === "error"}
      <circle cx="12" cy="12" r="9" /><path d="m9 9 6 6m0-6-6 6" />
    {:else}
      <circle cx="12" cy="12" r="9" /><path d="M8 12h.01M12 12h.01M16 12h.01" />
    {/if}
  </svg>
</button>

<div bind:this={menu} id={`${id}-model-status`} class="diagnostics" popover="auto" role="dialog"
  data-help-context="chat"
  aria-labelledby={`${id}-model-status-title`} tabindex="-1" onbeforetoggle={menuToggle}
  onkeydown={(event) => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); closeMenu(); }
  }}>
  <header>
    <h2 id={`${id}-model-status-title`}>Model &amp; index status</h2>
    <button bind:this={closeButton} type="button" aria-label="Close model status" onclick={() => closeMenu()}>Close</button>
  </header>
  <div class="diagnostic-content">
    <p class="status-label" data-status={statusKind}>{statusLabel}</p>
    {#if provider}<p><strong>{provider.label}</strong> · {provider.detail}</p>{/if}
    {#if checking || loading}<p role="status">Checking model &amp; index status…</p>{/if}
    {#if disconnected}<p>Connect a model in Settings to send questions. Notes and exact source retrieval do not require a semantic index.</p>{/if}
    {#each index?.runtime_warnings ?? [] as runtimeWarning}
      <p class="runtime-warning" role="status">{runtimeWarning}</p>
    {/each}
    {#if needsRecovery}
      <p class="runtime-warning">A native model needs recovery approval before using GPU again.</p>
      {#each index?.runtime_recovery ?? [] as recovery}
        <p class="runtime-warning"><strong>{recovery.label}</strong>: {recovery.reason}</p>
      {/each}
      <button type="button" onclick={openSettings}>Review recovery in Settings</button>
    {/if}
    {#if index}
      <p>{index.indexed_chunks} indexed chunks · {index.total_blocks} blocks · {index.pending_pages} pages pending</p>
      {#if !index.indexed_chunks}<p>No semantic index yet. Page and block questions still use exact source retrieval.</p>{/if}
      {#if !index.embedder_ready}<p>Configure an embedding model to index notes and use semantic search.</p>{/if}
      <button type="button" disabled={indexing || !index.embedder_ready} onclick={buildIndex}>{indexing ? "Indexing…" : "Index now"}</button>
      {#if cpuFallback}
        <p>Running on CPU despite GPU support. Retrying checks memory again; it cannot bypass the safety checks.</p>
        {#if running}<p>GPU retry is unavailable until the current request, including cancellation, has finished.</p>{/if}
        <button type="button" disabled={retrying || running || needsRecovery} onclick={retryGpu}>{retrying ? "Retrying…" : "Retry on GPU"}</button>
      {/if}
    {/if}
    {#if statusError}<p role="alert">{statusError}</p>{/if}
    {#if error}<p role="alert">{error}</p>{/if}
    {#if statusError || error}<button type="button" disabled={loading || running} onclick={refresh}>Retry status</button>{/if}
    {#if notice}<p class:runtime-warning={indexFailed} role="status">{notice}</p>{/if}
    <button type="button" onclick={openSettings}>Configure provider</button>
    <details class="privacy-note">
      <summary>Model &amp; web privacy</summary>
      {#if noNotesExcludesHistory}
        <p>No notes excludes earlier note-backed answers from the next request. They remain visible in this transcript.</p>
      {/if}
      <p>Prompts and selected excerpts go to the configured model. For web modes, Grafium contacts search engines and websites, then forwards results to your model server or service—even if that server has no internet. Endpoint location alone does not guarantee privacy. Grafium never switches models automatically.</p>
    </details>
  </div>
</div>

<style>
  .model-status-trigger { display: inline-grid; place-items: center; flex: 0 0 32px; width: 32px; height: 32px; padding: 0; color: var(--text-secondary); }
  .model-status-trigger[data-status="ready"] { color: var(--success, var(--text-primary)); }
  .model-status-trigger[data-status="working"] { color: var(--accent); }
  [data-status="warning"], .runtime-warning { color: var(--warning, #a76d16); }
  [data-status="error"], [role="alert"] { color: var(--danger, #c0392b); }
  .diagnostics { position: fixed; inset: auto; box-sizing: border-box; margin: 0; width: 360px; max-width: calc(100vw - 16px); max-height: calc(100dvh - 16px); overflow: auto; overscroll-behavior: contain; padding: 12px; background: var(--bg-secondary); border: 1px solid var(--border); border-radius: 8px; color: var(--text-secondary); font-size: 12px; text-align: left; box-shadow: 0 8px 28px rgb(0 0 0 / .25); }
  header { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 10px; }
  h2 { font-size: 13px; color: var(--text-primary); margin: 0; }
  .diagnostic-content { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
  .privacy-note { flex-basis: 100%; border-top: 1px solid var(--border); padding-top: 8px; }
  summary { cursor: pointer; color: var(--text-primary); }
  .privacy-note p { margin-top: 8px; }
  p { flex-basis: 100%; margin: 0; line-height: 1.5; overflow-wrap: anywhere; }
  button { font: inherit; background: var(--bg-primary); color: var(--text-primary); border: 1px solid var(--border); border-radius: 5px; padding: 5px 8px; cursor: pointer; }
  button:hover:not(:disabled) { background: var(--bg-hover); }
  button:disabled { opacity: .55; cursor: default; }
  button:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
