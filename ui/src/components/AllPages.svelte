<script lang="ts">
  import { SvelteMap } from "svelte/reactivity";
  import { countPages, listPagesWindow, createPage, deletePage } from "../lib/api";
  import type { Page } from "../lib/api";

  interface Props {
    onNavigate: (title: string) => void;
  }

  let { onNavigate }: Props = $props();

  // Fixed-height virtual list tuned for millions of rows: only the rows in (or
  // near) the viewport are ever in the DOM, and each window is fetched from the
  // DB on demand via offset paging off a partial index (~20ms even at offset
  // 900k). Nothing is capped — the full data set is browsable.
  const ROW_H = 44; // px per row, must match .page-row height
  const CHUNK = 200; // rows fetched per DB request
  const OVERSCAN = 8; // extra rows rendered above/below the viewport

  let total = $state(0);
  let sortByTitle = $state(false); // false = Recent (updated_at), true = A-Z (title)
  let newPageTitle = $state("");

  // Loaded rows keyed by absolute index; SvelteMap is reactive so the template
  // updates as windows stream in.
  let rows = new SvelteMap<number, Page>();
  const requested = new Set<number>(); // chunk indices already fetched
  let reloadToken = $state(0); // bump to force a re-fetch of the visible window

  let spacerEl: HTMLDivElement | null = $state(null);
  let relTop = $state(0); // px of list scrolled above the viewport top
  let visH = $state(0); // viewport height in px

  let startIndex = $derived(Math.max(0, Math.floor(relTop / ROW_H) - OVERSCAN));
  let endIndex = $derived(
    Math.min(total, Math.ceil((relTop + visH) / ROW_H) + OVERSCAN)
  );
  let visible = $derived.by(() => {
    const out: number[] = [];
    for (let i = startIndex; i < endIndex; i++) out.push(i);
    return out;
  });

  $effect(() => {
    void refreshCount();
  });

  // Track scroll/resize of the enclosing .main-content scroller.
  $effect(() => {
    if (!spacerEl) return;
    const parent = spacerEl.closest(".main-content") as HTMLElement | null;
    if (!parent) return;
    const update = () => {
      const p = parent.getBoundingClientRect();
      const s = spacerEl!.getBoundingClientRect();
      relTop = p.top - s.top;
      visH = parent.clientHeight;
    };
    update();
    parent.addEventListener("scroll", update, { passive: true });
    const ro = new ResizeObserver(update);
    ro.observe(parent);
    return () => {
      parent.removeEventListener("scroll", update);
      ro.disconnect();
    };
  });

  // Fetch whatever windows the visible range needs.
  $effect(() => {
    reloadToken;
    startIndex;
    endIndex;
    sortByTitle;
    ensureVisibleLoaded();
  });

  function ensureVisibleLoaded() {
    if (endIndex <= startIndex) return;
    const byTitle = sortByTitle;
    const firstChunk = Math.floor(startIndex / CHUNK);
    const lastChunk = Math.floor((endIndex - 1) / CHUNK);
    for (let ch = firstChunk; ch <= lastChunk; ch++) {
      if (requested.has(ch)) continue;
      requested.add(ch);
      void fetchChunk(ch, byTitle);
    }
  }

  async function fetchChunk(chunk: number, byTitle: boolean) {
    const offset = chunk * CHUNK;
    try {
      const pages = await listPagesWindow(CHUNK, offset, byTitle);
      // Ignore late responses from a superseded sort.
      if (byTitle !== sortByTitle) return;
      for (let k = 0; k < pages.length; k++) rows.set(offset + k, pages[k]);
    } catch (e) {
      requested.delete(chunk); // allow a retry
      console.error("Failed to load page window:", e);
    }
  }

  async function refreshCount() {
    try {
      total = await countPages();
    } catch (e) {
      console.error("Failed to count pages:", e);
    }
  }

  function resetWindows() {
    rows.clear();
    requested.clear();
    reloadToken++;
  }

  function setSort(byTitle: boolean) {
    if (byTitle === sortByTitle) return;
    sortByTitle = byTitle;
    resetWindows();
  }

  async function handleCreatePage() {
    const title = newPageTitle.trim();
    if (!title) return;
    await createPage(title);
    newPageTitle = "";
    resetWindows();
    await refreshCount();
  }

  async function handleDeletePage(page: Page) {
    await deletePage(page.id);
    resetWindows();
    await refreshCount();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") handleCreatePage();
  }

  function fmtDate(ts: number): string {
    return new Date(ts).toLocaleDateString();
  }
</script>

<div class="all-pages">
  <div class="header">
    <h1 class="page-title">All Pages</h1>
    <span class="count">{total.toLocaleString()} pages</span>
  </div>

  <div class="controls">
    <div class="new-page">
      <input
        type="text"
        placeholder="New page title..."
        bind:value={newPageTitle}
        onkeydown={handleKeydown}
        class="new-page-input"
      />
      <button onclick={handleCreatePage} class="btn-create">Create</button>
    </div>
    <button onclick={() => onNavigate("__import_media__")} class="btn-import-media" title="Import from video/audio (URL or file)">
      Import Media
    </button>
    <div class="sort-controls">
      <button class="sort-btn" class:active={!sortByTitle} onclick={() => setSort(false)}>Recent</button>
      <button class="sort-btn" class:active={sortByTitle} onclick={() => setSort(true)}>A-Z</button>
    </div>
  </div>

  {#if total === 0}
    <div class="empty-state">
      <p>No pages yet. Create one above!</p>
    </div>
  {:else}
    <div class="pages-spacer" bind:this={spacerEl} style="height: {total * ROW_H}px;">
      {#each visible as i (i)}
        {@const page = rows.get(i)}
        <div class="page-row" style="top: {i * ROW_H}px;">
          {#if page}
            <button class="page-link" onclick={() => onNavigate(page.title)}>
              {page.title}
              {#if page.is_journal}
                <span class="badge">Journal</span>
              {/if}
            </button>
            <span class="page-date">{fmtDate(page.updated_at)}</span>
            <button class="btn-delete" onclick={() => handleDeletePage(page)} title="Delete">×</button>
          {:else}
            <span class="page-link placeholder">…</span>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .all-pages {
    max-width: 800px;
    margin: 0 auto;
    padding: 40px 24px;
  }

  .header {
    display: flex;
    align-items: baseline;
    gap: 12px;
    margin-bottom: 24px;
  }

  .page-title {
    font-size: 32px;
    font-weight: 700;
    margin: 0;
    color: var(--text-primary);
  }

  .count {
    font-size: 13px;
    color: var(--text-muted);
  }

  .controls {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 20px;
    gap: 12px;
  }

  .new-page {
    display: flex;
    gap: 8px;
    flex: 1;
  }

  .new-page-input {
    flex: 1;
    padding: 8px 12px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: 14px;
    outline: none;
  }

  .new-page-input:focus {
    border-color: var(--accent);
  }

  .btn-create {
    padding: 8px 16px;
    background: var(--accent);
    color: white;
    border: none;
    border-radius: 6px;
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
  }

  .btn-create:hover {
    opacity: 0.9;
  }

  .sort-controls {
    display: flex;
    gap: 4px;
  }

  .btn-import-media {
    padding: 8px 16px;
    background: var(--btn-bg, transparent);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-secondary);
    font-size: 14px;
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
  }

  .btn-import-media:hover {
    background: var(--btn-bg-hover);
    color: var(--text-primary);
    border-color: var(--accent);
  }

  .sort-btn {
    padding: 6px 12px;
    background: none;
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text-secondary);
    font-size: 12px;
    cursor: pointer;
  }

  .sort-btn.active {
    background: var(--bg-active);
    color: var(--text-primary);
    border-color: var(--accent);
  }

  .pages-spacer {
    position: relative;
    width: 100%;
  }

  .page-row {
    position: absolute;
    left: 0;
    right: 0;
    height: 44px;
    box-sizing: border-box;
    display: flex;
    align-items: center;
    padding: 8px 12px;
    border-radius: 6px;
    gap: 12px;
  }

  .page-row:hover {
    background: var(--bg-hover);
  }

  .page-link {
    flex: 1;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: 15px;
    cursor: pointer;
    text-align: left;
    display: flex;
    align-items: center;
    gap: 8px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .page-link:hover {
    color: var(--accent);
  }

  .page-link.placeholder {
    color: var(--text-muted);
    cursor: default;
  }

  .badge {
    font-size: 10px;
    padding: 2px 6px;
    background: var(--bg-secondary);
    border-radius: 4px;
    color: var(--text-muted);
  }

  .page-date {
    font-size: 12px;
    color: var(--text-muted);
    white-space: nowrap;
  }

  .btn-delete {
    background: none;
    border: none;
    color: var(--text-muted);
    font-size: 18px;
    cursor: pointer;
    padding: 2px 6px;
    border-radius: 4px;
    opacity: 0;
  }

  .page-row:hover .btn-delete {
    opacity: 1;
  }

  .btn-delete:hover {
    background: var(--danger-bg);
    color: var(--danger);
  }

  .empty-state {
    text-align: center;
    padding: 60px 20px;
    color: var(--text-muted);
  }
</style>
