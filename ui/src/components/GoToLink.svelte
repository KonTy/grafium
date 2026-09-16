<script lang="ts">
  import { onMount } from "svelte";
  import { listPageSummaries, type PageSummary } from "../lib/api";
  import { fuzzyRank } from "../lib/fuzzy";
  import { retainEditorForDialog } from "../lib/editorDialogFocus";

  let { onSelect, onCancel }: {
    onSelect: (page: PageSummary) => void;
    onCancel: () => void;
  } = $props();

  const id = $props.id();
  const rowHeight = 44;
  let dialog: HTMLDialogElement;
  let input: HTMLInputElement;
  let list: HTMLDivElement;
  let query = $state("");
  let pages = $state<PageSummary[]>([]);
  let loading = $state(true);
  let error = $state("");
  let selectedIndex = $state(0);
  let scrollTop = $state(0);
  let listHeight = $state(352);
  let mounted = false;
  let request = 0;

  const ranked = $derived(query.trim() ? fuzzyRank(pages, query, (page) => page.title) : pages);
  const start = $derived(Math.max(0, Math.floor(scrollTop / rowHeight) - 4));
  const end = $derived(Math.min(ranked.length, Math.ceil((scrollTop + listHeight) / rowHeight) + 4));

  $effect(() => {
    ranked;
    selectedIndex = 0;
    scrollTop = 0;
    if (list) list.scrollTop = 0;
  });

  onMount(() => {
    mounted = true;
    const active = document.activeElement;
    const editor = active?.closest<HTMLElement>(".cm-content")?.closest<HTMLElement>(".cm-editor");
    const releaseEditor = editor ? retainEditorForDialog(editor) : undefined;
    dialog.showModal();
    input.focus({ preventScroll: true });
    const observer = new ResizeObserver(() => { listHeight = list.clientHeight; });
    observer.observe(list);
    void loadPages();
    return () => {
      mounted = false;
      observer.disconnect();
      dialog.close();
      releaseEditor?.();
    };
  });

  async function loadPages() {
    const current = ++request;
    loading = true;
    error = "";
    try {
      const result = await listPageSummaries();
      if (mounted && current === request) pages = result;
    } catch (cause) {
      if (mounted && current === request) {
        error = `Could not load pages: ${cause instanceof Error ? cause.message : String(cause)}`;
      }
    } finally {
      if (mounted && current === request) loading = false;
    }
  }

  function cancel() {
    dialog.close();
    onCancel();
  }

  function choose(page: PageSummary) {
    dialog.close();
    onSelect(page);
  }

  function moveSelection(next: number) {
    selectedIndex = Math.max(0, Math.min(ranked.length - 1, next));
    const top = selectedIndex * rowHeight;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (top + rowHeight > list.scrollTop + list.clientHeight) {
      list.scrollTop = top + rowHeight - list.clientHeight;
    }
    scrollTop = list.scrollTop;
  }

  function handleKeydown(event: KeyboardEvent) {
    event.stopPropagation();
    if (event.isComposing) return;
    if (event.key === "Tab") {
      const controls = [...dialog.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled)")]
        .filter((element) => element.tabIndex >= 0 && element.getClientRects().length > 0);
      const first = controls[0];
      const last = controls[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
      return;
    }
    if ((event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey
      && (event.code === "KeyL" || event.key.toLowerCase() === "l")) {
      event.preventDefault();
      input.focus();
      input.select();
      return;
    }
    if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
    if (event.key === "Escape") {
      event.preventDefault();
      cancel();
      return;
    }
    if (event.target !== input) return;
    const step = Math.max(1, Math.floor(list.clientHeight / rowHeight));
    const moves: Record<string, number | undefined> = {
      ArrowDown: selectedIndex + 1,
      ArrowUp: selectedIndex - 1,
      PageDown: selectedIndex + step,
      PageUp: selectedIndex - step,
    };
    const next = moves[event.key];
    if (next !== undefined) {
      event.preventDefault();
      moveSelection(next);
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (!loading && !error && ranked[selectedIndex]) choose(ranked[selectedIndex]);
    }
  }
</script>

<dialog
  bind:this={dialog}
  class="go-link-dialog"
  aria-labelledby={`${id}-title`}
  aria-describedby={`${id}-help`}
  onkeydown={handleKeydown}
  oncancel={(event) => { event.preventDefault(); cancel(); }}
  onclick={(event) => {
    const rect = dialog.getBoundingClientRect();
    if (event.target === dialog && (event.clientX < rect.left || event.clientX > rect.right
      || event.clientY < rect.top || event.clientY > rect.bottom)) cancel();
  }}
>
  <header>
    <h2 id={`${id}-title`}>Go to link</h2>
    <button class="close-button" type="button" aria-label="Close Go to link" title="Close (Esc)" onclick={cancel}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true">
        <path d="m6 6 12 12M18 6 6 18" />
      </svg>
    </button>
  </header>
  <div class="search">
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true">
      <circle cx="10.5" cy="10.5" r="6.5" /><path d="m16 16 5 5" />
    </svg>
    <input
      bind:this={input}
      bind:value={query}
      role="combobox"
      aria-label="Find a page"
      aria-autocomplete="list"
      aria-expanded="true"
      aria-controls={`${id}-results`}
      aria-activedescendant={!loading && !error && ranked.length ? `${id}-option-${selectedIndex}` : undefined}
      placeholder="Type a page name..."
      autocomplete="off"
      spellcheck="false"
    />
  </div>
  {#if loading}
    <p class="message" role="status">Loading pages...</p>
  {:else if error}
    <div class="message" role="alert">
      <p>{error}</p>
      <button type="button" onclick={() => { input.focus(); void loadPages(); }}>Retry</button>
    </div>
  {:else if !ranked.length}
    <p class="message" role="status">{pages.length ? "No matching pages. Try fewer letters." : "No pages in this graph yet."}</p>
  {/if}
  <div
    bind:this={list}
    class="results"
    id={`${id}-results`}
    role="listbox"
    aria-label="Pages"
    aria-busy={loading}
    onscroll={() => { scrollTop = list.scrollTop; }}
  >
    <div class="rows" role="presentation" style:height={`${ranked.length * rowHeight}px`}>
      {#each ranked.slice(start, end) as page, index (page.id)}
        {@render option(page, start + index)}
      {/each}
      {#if ranked[selectedIndex] && (selectedIndex < start || selectedIndex >= end)}
        {@render option(ranked[selectedIndex], selectedIndex)}
      {/if}
    </div>
  </div>
  <footer>
    <span role="status">{!loading && !error ? `${ranked.length.toLocaleString()} ${ranked.length === 1 ? "page" : "pages"}` : ""}</span>
    <span id={`${id}-help`}>Arrows to browse / Enter to open / Esc to close</span>
  </footer>
</dialog>

{#snippet option(page: PageSummary, index: number)}
  <button
    type="button"
    class="result"
    class:selected={index === selectedIndex}
    role="option"
    id={`${id}-option-${index}`}
    aria-selected={index === selectedIndex}
    aria-posinset={index + 1}
    aria-setsize={ranked.length}
    tabindex="-1"
    title={page.title}
    style:top={`${index * rowHeight}px`}
    style:height={`${rowHeight}px`}
    onmousedown={(event) => event.preventDefault()}
    onclick={() => choose(page)}
  >
    <span class="page-title">{page.title}</span>
    <span class="page-kind">{page.is_journal ? "Journal" : "Page"}</span>
  </button>
{/snippet}

<style>
  .go-link-dialog {
    width: min(600px, calc(100vw - 32px));
    max-height: calc(100dvh - min(15dvh, 120px) - 24px);
    margin: min(15dvh, 120px) auto 0;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--surface-overlay);
    color: var(--text-primary);
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.4);
    overflow: hidden;
  }
  .go-link-dialog[open] { display: flex; flex-direction: column; }
  .go-link-dialog::backdrop { background: rgba(0, 0, 0, 0.6); }
  header { display: flex; align-items: center; justify-content: space-between; padding: 12px 16px 8px; }
  h2 { margin: 0; font-size: 16px; font-weight: 600; }
  button { cursor: pointer; color: inherit; }
  .close-button {
    display: inline-flex; align-items: center; justify-content: center;
    width: 36px; height: 36px; border: 0; border-radius: 6px; background: transparent;
  }
  .close-button:hover { background: var(--bg-hover); }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
  .search {
    display: flex; align-items: center; gap: 10px;
    margin: 0 16px 12px; border: 1px solid var(--border); border-radius: 6px;
    padding-inline: 12px; color: var(--text-secondary); background: var(--bg-primary);
  }
  .search:focus-within { border-color: var(--accent); outline: 1px solid var(--accent); }
  .search svg { flex-shrink: 0; }
  input {
    width: 100%; min-width: 0; height: 44px; padding: 0; border: 0; outline: none;
    background: transparent; color: var(--text-primary); font: inherit; font-size: 16px;
  }
  input::placeholder { color: var(--text-secondary); }
  .results { overflow-y: auto; overscroll-behavior: contain; min-height: 0; max-height: min(352px, 50dvh); }
  .rows { position: relative; }
  .result {
    position: absolute; inset-inline: 0; width: 100%; padding: 0 16px;
    display: flex; align-items: center; gap: 12px; text-align: start;
    border: 0; border-radius: 0; background: transparent; font: inherit; font-size: 14px;
  }
  .result:hover { background: var(--bg-hover); }
  .result.selected { background: var(--bg-active); }
  .page-title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .page-kind { flex-shrink: 0; color: var(--text-secondary); font-size: 12px; }
  .message { padding: 12px 16px 20px; margin: 0; color: var(--text-secondary); overflow-wrap: anywhere; }
  .message button {
    margin-top: 12px; padding: 6px 12px; border: 1px solid var(--border);
    border-radius: 6px; background: var(--bg-hover);
  }
  footer {
    display: flex; flex-wrap: wrap; gap: 6px 12px; justify-content: space-between;
    padding: 12px 16px; border-top: 1px solid var(--border); color: var(--text-secondary); font-size: 12px;
  }
</style>
