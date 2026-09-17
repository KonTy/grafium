<script lang="ts">
  import { tick } from "svelte";
  import { searchFts, searchPageTitles, getPage, type Block, type PageSummary } from "../lib/api";
  import { aiSearch, aiHealthCheck, type SemanticSearchResult } from "../lib/knowledge";
  import type { PageNavigationTarget } from "../lib/navigation";
  let { open = true, onClose, onNavigate, onOpenSettings = () => onNavigate("__settings__") }: {
    open?: boolean; onClose: () => void; onNavigate: (target: PageNavigationTarget) => void; onOpenSettings?: () => void;
  } = $props();

  type QuickMatch = { kind: "page"; page: PageSummary } | { kind: "block"; block: Block; pageTitle: string };
  type SearchMatch = { source: "quick"; match: QuickMatch } | { source: "ai"; result: SemanticSearchResult };
  let dialog: HTMLDialogElement;
  let input: HTMLInputElement;
  let query = $state("");
  let matches = $state<QuickMatch[]>([]);
  let semantic = $state<SemanticSearchResult[]>([]);
  let searching = $state(false);
  let semanticSearching = $state(false);
  let embedderAvailable = $state(false);
  let checking = $state(true);
  let error = $state("");
  let modelError = $state("");
  let selectedIndex = $state(-1);
  let hasSearched = $state(false);
  let version = 0;
  let semanticVersion = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const titles = new Map<string, string>();
  const combined = $derived<SearchMatch[]>([
    ...matches.map((match) => ({ source: "quick" as const, match })),
    ...semantic.map((result) => ({ source: "ai" as const, result })),
  ]);

  $effect(() => {
    if (!dialog) return;
    if (!open) {
      dialog.close();
      version++; semanticVersion++;
      clearTimeout(timer);
      searching = false; semanticSearching = false;
      return;
    }
    if (!dialog.open) dialog.showModal();
    void tick().then(() => input?.focus());
    let disposed = false;
    checking = true;
    modelError = "";
    void aiHealthCheck().then((health) => { if (!disposed) embedderAvailable = health.enabled && health.embedder_available; })
      .catch((cause) => {
        if (!disposed) {
          embedderAvailable = false;
          modelError = `Could not check the embedding model: ${String(cause)}. Text search is still available.`;
        }
      })
      .finally(() => { if (!disposed) checking = false; });
    return () => { disposed = true; clearTimeout(timer); };
  });
  $effect(() => {
    const index = selectedIndex;
    if (index >= 0) document.getElementById(`global-search-result-${index}`)?.scrollIntoView({ block: "nearest" });
  });

  async function runQuickSearch(text: string, request: number) {
    try {
      const [pages, blocks] = await Promise.all([searchPageTitles(text, 8), searchFts(text, 15)]);
      if (request !== version) return;
      for (const page of pages) titles.set(page.id, page.title);
      const blockMatches = await Promise.all(blocks.slice(0, 12).map(async (block): Promise<QuickMatch> => {
        let pageTitle = titles.get(block.page_id);
        if (!pageTitle) {
          pageTitle = (await getPage({ id: block.page_id })).title;
          titles.set(block.page_id, pageTitle);
        }
        return { kind: "block", block, pageTitle };
      }));
      if (request !== version) return;
      matches = [...pages.map((page) => ({ kind: "page" as const, page })), ...blockMatches];
      hasSearched = true;
    } catch (cause) { if (request === version) error = `Text search failed: ${String(cause)}`; }
    finally { if (request === version) searching = false; }
  }
  function queueSearch(immediate = false) {
    clearTimeout(timer);
    const text = query.trim();
    const request = ++version;
    semanticVersion++;
    semanticSearching = false;
    matches = []; semantic = []; selectedIndex = -1; hasSearched = false; error = "";
    searching = !!text;
    if (!text) return;
    if (immediate) void runQuickSearch(text, request);
    else timer = setTimeout(() => { void runQuickSearch(text, request); }, 120);
  }
  async function semanticSearch() {
    if (!query.trim() || !embedderAvailable || semanticSearching) return;
    const request = ++semanticVersion;
    const question = query.trim();
    semanticSearching = true; error = ""; selectedIndex = -1;
    try {
      const results = await aiSearch(question, 20);
      if (request === semanticVersion) semantic = results;
    } catch (cause) { if (request === semanticVersion) error = `AI Search failed: ${String(cause)}`; }
    finally { if (request === semanticVersion) semanticSearching = false; }
  }
  function navigate(entry: SearchMatch) {
    onClose();
    if (entry.source === "ai") {
      if (entry.result.block_id) window.dispatchEvent(new CustomEvent("navigate-page", { detail: { pageName: entry.result.page_title, pageId: entry.result.page_id, targetBlockId: entry.result.block_id } }));
      else onNavigate({ id: entry.result.page_id });
    } else if (entry.match.kind === "page") onNavigate({ id: entry.match.page.id });
    else window.dispatchEvent(new CustomEvent("navigate-page", { detail: { pageName: entry.match.pageTitle, pageId: entry.match.block.page_id, targetBlockId: entry.match.block.id } }));
  }
  function handleKeydown(event: KeyboardEvent) {
    if (event.isComposing) return;
    if (event.key === "ArrowDown") {
      event.preventDefault(); selectedIndex = Math.min(selectedIndex + 1, combined.length - 1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault(); selectedIndex = Math.max(selectedIndex - 1, -1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (combined.length) navigate(combined[Math.max(0, selectedIndex)]);
      else queueSearch(true);
    }
  }
</script>

<dialog bind:this={dialog} class="global-search-dialog" aria-labelledby="global-search-title"
  oncancel={(event) => { event.preventDefault(); onClose(); }}
  onclick={(event) => { if (event.target === dialog) { const box = dialog.getBoundingClientRect(); if (event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom) onClose(); } }}>
  <header><h2 id="global-search-title">Search your graph</h2><button class="close" onclick={onClose} aria-label="Close search">×</button></header>
  <form onsubmit={(event) => { event.preventDefault(); queueSearch(true); }}>
    <input bind:this={input} bind:value={query} oninput={() => queueSearch()} onkeydown={handleKeydown}
      aria-label="Search pages and blocks" aria-controls="global-search-results"
      aria-activedescendant={selectedIndex >= 0 ? `global-search-result-${selectedIndex}` : undefined}
      placeholder="Search page titles and note text…" autocomplete="off" />
    <div class="search-actions">
      <button type="submit" disabled={!query.trim()}>{searching ? "Searching…" : "Search"}</button>
      <button type="button" onclick={semanticSearch} disabled={!query.trim() || semanticSearching || !embedderAvailable}
        title="Semantic search uses your configured embedding model. Text search does not need AI.">{semanticSearching ? "AI searching…" : "AI Search"}</button>
      <span>↑ ↓ to browse · Enter to open · Esc to close</span>
    </div>
  </form>
  <div class="results" id="global-search-results">
    {#if modelError}<p class="error" role="alert">{modelError}</p>
    {:else if !checking && !embedderAvailable}<p>Text search works without AI. <button class="text-button" onclick={() => { onClose(); onOpenSettings(); }}>Configure an embedding model</button> for semantic AI Search.</p>{/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if !query.trim()}<p>Find a page or passage by typing a title, phrase, or word prefix.</p>
    {:else if searching}<p class="shimmer" role="status">Searching note titles and text…</p>
    {:else if hasSearched && !combined.length}<p>No matches. Try a shorter word or a different phrase.</p>{/if}
    {#each combined as entry, index}
      {#if index === 0 && matches.length}<h3>Quick matches</h3>{/if}
      {#if index === matches.length && semantic.length}<h3>AI semantic results</h3>{/if}
      <button class="search-result" class:selected={selectedIndex === index} id={`global-search-result-${index}`} onclick={() => navigate(entry)}>
        {#if entry.source === "ai"}
          <span class="result-header"><strong>{entry.result.page_title}</strong><span>{Math.round(entry.result.score * 100)}%</span></span>
          <span class="snippet">{entry.result.content.slice(0, 200)}</span>
        {:else if entry.match.kind === "page"}
          <span class="result-header"><span class="kind">Page</span><strong>{entry.match.page.title}</strong></span>
        {:else}
          <span class="result-header"><span class="kind">Block</span><strong>{entry.match.pageTitle}</strong></span>
          <span class="snippet">{entry.match.block.content.replace(/^[-*>\s#]+/, "").slice(0, 160) || "(empty block)"}</span>
        {/if}
      </button>
    {/each}
  </div>
</dialog>

<style>
  .global-search-dialog { box-sizing: border-box; width: min(680px, calc(100vw - 24px)); max-height: min(720px, calc(100dvh - 32px)); padding: 16px; border: 1px solid var(--border); border-radius: 8px; background: var(--bg-secondary); color: var(--text-primary); box-shadow: 0 12px 40px rgb(0 0 0 / .25); }
  .global-search-dialog[open] { display: flex; flex-direction: column; gap: 12px; }
  .global-search-dialog::backdrop { background: rgb(0 0 0 / .4); }
  header { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
  h2 { font-size: 16px; margin: 0; }
  h3 { font-size: 12px; color: var(--text-secondary); margin: 16px 0 4px; }
  form { display: grid; gap: 8px; }
  input { box-sizing: border-box; width: 100%; font: inherit; font-size: 14px; padding: 10px; border: 1px solid var(--border); border-radius: 5px; color: var(--text-primary); background: var(--bg-primary); }
  input::placeholder { color: var(--text-secondary); }
  button { cursor: pointer; font: inherit; font-size: 13px; color: var(--text-primary); background: var(--bg-primary); border: 1px solid var(--border); border-radius: 5px; padding: 6px 10px; }
  button:hover:not(:disabled), .search-result.selected { background: var(--bg-hover); }
  button:disabled { opacity: .55; cursor: default; }
  button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .close { border: 0; background: transparent; font-size: 22px; padding: 0 6px; }
  .search-actions { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
  .search-actions > span { font-size: 11px; color: var(--text-secondary); }
  .results { min-height: 0; overflow-y: auto; display: flex; flex-direction: column; gap: 6px; overscroll-behavior: contain; }
  .search-result { text-align: left; padding: 10px; flex-shrink: 0; }
  .search-result.selected { border-color: var(--accent); }
  .result-header { display: flex; align-items: baseline; gap: 8px; overflow-wrap: anywhere; }
  .result-header strong { flex: 1; min-width: 0; }
  .kind { font-size: 11px; color: var(--text-secondary); }
  .snippet { display: block; margin-top: 5px; line-height: 1.5; color: var(--text-secondary); overflow-wrap: anywhere; }
  p { margin: 4px 0; font-size: 12px; line-height: 1.5; color: var(--text-secondary); }
  .text-button { border: 0; padding: 0; background: transparent; color: var(--accent); font-size: inherit; text-decoration: underline; }
  .error { color: var(--danger, #c0392b); }
</style>
