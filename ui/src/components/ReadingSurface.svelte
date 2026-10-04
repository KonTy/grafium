<script lang="ts">
  import { tick, type Snippet } from "svelte";
  import { enterReaderFullscreen, type ReaderFullscreen } from "../lib/readerFullscreen";
  import { hasKeyboardOverlay } from "../lib/mainPaneScroll";
  import { showToast } from "../lib/toast.svelte";
  import { readerNavigationKey, readerOwnsNavigation, type ReaderTurn } from "../lib/readerNavigation";
  import ReaderMenu from "./ReaderMenu.svelte";
  import { shortcutTitle } from "../lib/shortcuts";

  let { children, navigation, actions, bookmarks, bookmarkCount = 0, onBack, onBookmark, onNavigate }: {
    children: Snippet; navigation: Snippet; actions?: Snippet; bookmarks?: Snippet;
    bookmarkCount?: number;
    onBack?: () => void; onBookmark?: () => void;
    onNavigate?: (direction: ReaderTurn) => void;
  } = $props();
  const id = $props.id();
  let root = $state<HTMLElement>();
  let handle = $state<HTMLButtonElement>();
  let controlsOpen = $state(false);
  let bookmarksOpen = $state(false);
  let bookmarkDetails = $state<HTMLDetailsElement>();
  let previousBookmarkCount: number | undefined;
  let expanded = $state(false);
  let busy = $state(false);
  let error = $state("");
  let session: ReaderFullscreen | undefined;
  let disposed = false;

  export function toggleControls() {
    controlsOpen = !controlsOpen;
    void tick().then(() => {
      if (controlsOpen) root?.querySelector<HTMLButtonElement>(".reading-controls button:not(:disabled)")?.focus({ preventScroll: true });
      else handle?.focus({ preventScroll: true });
    });
  }
  export function bookmark() { onBookmark?.(); }
  export function revealBookmarks() {
    controlsOpen = true; bookmarksOpen = true;
    void tick().then(() => bookmarkDetails?.querySelector("li:last-child")?.scrollIntoView({ block: "nearest" }));
  }
  $effect(() => {
    const count = bookmarkCount;
    if (previousBookmarkCount !== undefined && count > previousBookmarkCount) revealBookmarks();
    previousBookmarkCount = count;
  });
  export async function exitFullscreen() {
    if (!session || busy) return;
    busy = true;
    try {
      await session.exit();
      session = undefined; expanded = false;
    } catch (cause) {
      error = `Could not leave fullscreen: ${String(cause)}`;
      if (disposed) showToast(error, "error");
    } finally { busy = false; }
  }
  export async function toggleFullscreen() {
    if (busy || !root) return;
    if (session) { await exitFullscreen(); return; }
    busy = true; error = "";
    try {
      const opened = await enterReaderFullscreen(root);
      if (disposed) { await opened.exit(); return; }
      session = opened; expanded = true; controlsOpen = false;
    } catch (cause) {
      error = `Could not enter fullscreen: ${String(cause)}`;
      if (disposed) showToast(error, "error");
    } finally { busy = false; }
  }
  export async function dismiss() {
    if (session) await exitFullscreen();
    else if (controlsOpen) toggleControls();
  }
  async function fullscreenChanged() {
    const current = session;
    if (!current || busy) return;
    try {
      if (!await current.isActive() && session === current) {
        session = undefined; expanded = false;
      }
    } catch (cause) { error = `Could not check fullscreen: ${String(cause)}`; }
  }
  function keydown(event: KeyboardEvent) {
    if (event.defaultPrevented || event.isComposing || hasKeyboardOverlay(document)) return;
    if (onNavigate && root && readerOwnsNavigation(event, root)) {
      const direction = readerNavigationKey(event, document)!;
      event.preventDefault(); event.stopImmediatePropagation(); onNavigate(direction);
    } else if (event.key === "F8" && !event.ctrlKey && !event.metaKey && !event.altKey) {
      event.preventDefault(); event.stopImmediatePropagation(); toggleControls();
    } else if (event.key === "F11" && !event.ctrlKey && !event.metaKey && !event.altKey) {
      event.preventDefault(); event.stopImmediatePropagation(); void toggleFullscreen();
    } else if (event.key === "Escape" && (expanded || controlsOpen)) {
      event.preventDefault(); event.stopImmediatePropagation();
      if (expanded) void exitFullscreen();
      else toggleControls();
    }
  }
  $effect(() => {
    window.addEventListener("keydown", keydown, true);
    window.addEventListener("resize", fullscreenChanged);
    document.addEventListener("fullscreenchange", fullscreenChanged);
    return () => {
      disposed = true;
      window.removeEventListener("keydown", keydown, true);
      window.removeEventListener("resize", fullscreenChanged);
      document.removeEventListener("fullscreenchange", fullscreenChanged);
      if (session) void session.exit().catch(cause => showToast(`Could not restore window mode: ${String(cause)}`, "error"));
    };
  });
</script>

<section bind:this={root} class="reading-surface" class:expanded aria-label="Reading surface">
  <div class="reading-page">{@render children()}</div>
  <button bind:this={handle} class="controls-handle" aria-label={controlsOpen ? "Hide reading controls" : "Show reading controls"}
    aria-expanded={controlsOpen} aria-controls={`${id}-controls`} aria-keyshortcuts="F8"
    title="Reading controls (F8)" onclick={toggleControls}>
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
      <path d={controlsOpen ? "m6 6 12 12M6 18 18 6" : "M4 6h16M4 12h16M4 18h16"} />
    </svg>
  </button>
  <div id={`${id}-controls`} class="reading-controls" hidden={!controlsOpen}>
    <nav aria-label="Reading controls">
      {@render navigation()}
      {#if onBookmark}<button title={shortcutTitle("Bookmark", "bookmark")} onclick={onBookmark}>Bookmark</button>{/if}
      {#if bookmarks}<button aria-expanded={bookmarksOpen} aria-controls={`${id}-bookmarks`}
        onclick={() => { if (bookmarksOpen) bookmarksOpen = false; else revealBookmarks(); }}>Bookmarks ({bookmarkCount})</button>{/if}
      <button title="Fullscreen (F11; Escape to leave)" disabled={busy} aria-pressed={expanded} aria-keyshortcuts="F11" onclick={() => { void toggleFullscreen(); }}>
        {expanded ? "Exit fullscreen" : "Fullscreen"}
      </button>
      {#if onBack}<button onclick={async () => { await exitFullscreen(); if (!session) onBack?.(); }}>Library</button>{/if}
      {#if actions}
        <ReaderMenu label="More reading actions" heading="Reading actions" closeOnAction>
          {#snippet children()}{@render actions()}{/snippet}
        </ReaderMenu>
      {/if}
    </nav>
    {#if bookmarks}<details id={`${id}-bookmarks`} bind:this={bookmarkDetails} bind:open={bookmarksOpen}>
      <summary>Bookmarks ({bookmarkCount})</summary>{@render bookmarks()}
    </details>{/if}
  </div>
  {#if error}<div class="fullscreen-error" role="alert">{error}</div>{/if}
</section>

<style>
  .reading-surface { position: relative; display: flex; flex: 1; min-width: 0; min-height: 0; height: 100%; color: var(--text-primary); background: var(--bg-primary); }
  .reading-surface.expanded { position: fixed; inset: 0; z-index: 10000; width: 100%; height: 100dvh; }
  .reading-surface:fullscreen { width: 100%; height: 100%; }
  .reading-page { display: flex; flex-direction: column; flex: 1; min-width: 0; min-height: 0; }
  .controls-handle { position: absolute; right: max(8px, env(safe-area-inset-right)); bottom: max(8px, env(safe-area-inset-bottom)); width: 44px; height: 44px; display: grid; place-items: center; opacity: .55; z-index: 2; }
  .controls-handle:hover, .controls-handle:focus-visible, .controls-handle[aria-expanded="true"] { opacity: 1; }
  .reading-controls { position: absolute; z-index: 1; inset: auto 0 0; max-height: min(75%, 600px); overflow-y: auto; padding: 12px max(62px, env(safe-area-inset-right)) max(12px, env(safe-area-inset-bottom)) max(12px, env(safe-area-inset-left)); background: var(--bg-secondary); border-top: 1px solid var(--border); box-shadow: 0 -4px 18px #0003; }
  .reading-controls[hidden] { display: none; }
  nav { position: sticky; top: -12px; z-index: 3; display: flex; align-items: center; gap: 8px; flex-wrap: wrap; padding: 6px 0; background: var(--bg-secondary); }
  button { font: inherit; color: var(--text-primary); background: var(--bg-primary); border: 1px solid var(--border); border-radius: 6px; padding: 8px 10px; min-height: 40px; cursor: pointer; }
  button:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  button:disabled { opacity: .5; cursor: default; }
  details { margin-top: 12px; } summary { cursor: pointer; padding: 6px 0; }
  .fullscreen-error { position: absolute; top: 0; left: 0; right: 0; padding: 10px; background: var(--bg-primary); color: var(--danger, var(--text-primary)); }
</style>
