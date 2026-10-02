<script lang="ts">
  import { untrack } from "svelte";
  import AssistantConversation from "./AssistantConversation.svelte";
  import ChatSwitcher from "./ChatSwitcher.svelte";
  import { getGraphInfo } from "../lib/api";
  import {
    getGlobalConversation, getAssistantConversation, restoreAssistantConversations,
    type AssistantThread,
  } from "../lib/assistantConversations";
  import type { PageNavigationTarget } from "../lib/navigation";
  import { chatLayout, CHAT_HISTORY_MIN } from "../lib/chatLayout";
  import { showToast } from "../lib/toast.svelte";

  let { active = true, conversationId = null, readingInset = 0, onOpenSettings = () => {}, onNavigate = () => {}, onFindLinks }: {
    active?: boolean; conversationId?: string | null; onOpenSettings?: () => void;
    readingInset?: number;
    onNavigate?: (target: PageNavigationTarget) => void;
    onFindLinks?: (page: { id: string; title: string }, exactOnly?: boolean) => void;
  } = $props();
  let thread = $state.raw<AssistantThread | null>(null);
  let error = $state("");
  let ready = $state(false);
  let graphPath = $state("");
  let hostWidth = $state(0);
  let historyOffset = $state(0);
  let host = $state<HTMLDivElement>();
  let drawerToggle = $state<HTMLButtonElement>();
  let resizing = $state(false);
  let drawerOpen = $state(false);
  const layout = $derived(chatLayout(hostWidth, historyOffset, readingInset));
  const widthKey = "grafium.chat.historyWidthOffset";
  $effect(() => {
    if (!active || layout.drawer) resizing = false;
    if (!active || !layout.drawer) drawerOpen = false;
  });
  $effect(() => {
    try {
      const saved = localStorage.getItem(widthKey);
      if (saved !== null) {
        const value = Number(saved);
        if (Number.isFinite(value)) historyOffset = value;
        else console.warn("Invalid saved Chat history width; using the default.");
      }
    } catch (cause) {
      console.error("Could not restore Chat history width", cause);
    }
  });
  function saveWidth() {
    try { localStorage.setItem(widthKey, String(historyOffset)); }
    catch (cause) {
      console.error("Could not save Chat history width", cause);
      showToast("Could not save the conversation panel width.", "error");
    }
  }
  function setWidth(width: number) {
    historyOffset = Math.max(CHAT_HISTORY_MIN, Math.min(layout.maximum, width)) - layout.automatic;
  }
  function resizeKey(event: KeyboardEvent) {
    const step = event.shiftKey ? 50 : 20;
    const width = event.key === "ArrowLeft" ? layout.history - step
      : event.key === "ArrowRight" ? layout.history + step
      : event.key === "Home" ? CHAT_HISTORY_MIN
      : event.key === "End" ? layout.maximum : null;
    if (width === null) return;
    event.preventDefault();
    event.stopPropagation();
    setWidth(width);
    saveWidth();
  }
  function finishResize(event: PointerEvent) {
    if (!resizing) return;
    resizing = false;
    if (event.currentTarget instanceof HTMLElement && event.currentTarget.hasPointerCapture(event.pointerId))
      event.currentTarget.releasePointerCapture(event.pointerId);
    saveWidth();
  }
  // Stored conversations are read once per graph, not once per mount: Chat is
  // opened and closed constantly and re-reading would flicker the list.
  let restored = "";
  $effect(() => {
    const id = conversationId;
    if (!active) return;
    let disposed = false;
    ready = false;
    void getGraphInfo().then(async (graph) => {
      if (disposed) return;
      graphPath = graph.path;
      if (restored !== graph.path) {
        restored = graph.path;
        await restoreAssistantConversations(graph.path);
        if (disposed) return;
      }
      const requested = id ? getAssistantConversation(id) : undefined;
      if (id && !requested) throw new Error("This conversation is no longer available. Open Chat to start a general conversation.");
      if (requested && requested.graphPath !== graph.path) throw new Error("Return to the original graph to open this conversation.");
      // Reopening Chat must not throw away the conversation the user picked
      // in the switcher, so a still-valid selection outranks the default.
      const picked = untrack(() => thread);
      const held = !id && picked && picked.graphPath === graph.path
        && getAssistantConversation(picked.id) ? picked : undefined;
      const next = requested ?? held ?? getGlobalConversation(graph.path);
      if (untrack(() => thread?.id) !== next.id) thread = next;
      error = "";
      ready = true;
    }).catch((cause) => { if (!disposed) error = `Could not open Chat: ${String(cause)}`; });
    return () => { disposed = true; };
  });
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key === "Escape" && drawerOpen) {
      drawerOpen = false;
      drawerToggle?.focus();
      event.stopPropagation();
    }
  }}
/>

<div class="chat-view" class:compact={layout.drawer} class:resizing
  bind:clientWidth={hostWidth} style:--chat-history-width={`${layout.history}px`}
  style:--chat-reading-inset={`${layout.inset}px`}>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if thread}
    <div class="chat-bar">
      <button
        type="button"
        class="chats-toggle"
        bind:this={drawerToggle}
        aria-expanded={drawerOpen}
        aria-controls="chat-switcher"
        onclick={() => (drawerOpen = !drawerOpen)}
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
          <path d="M3 6h18M3 12h18M3 18h18" />
        </svg>
        Chats
      </button>
    </div>
    <div class="conversation-host" bind:this={host} hidden={!ready} inert={!ready}>
      <ChatSwitcher
        {graphPath}
        currentId={thread.id}
        open={drawerOpen}
        drawer={layout.drawer}
        onSelect={(next) => { thread = next; error = ""; drawerOpen = false; }}
      />
      {#if !layout.drawer}
        <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (ARIA window splitters are focusable separators with keyboard controls.) -->
        <div class="history-resizer" role="separator" tabindex="0"
          aria-label="Resize conversations" aria-orientation="vertical"
          aria-controls="chat-switcher" aria-valuemin={CHAT_HISTORY_MIN}
          aria-valuemax={Math.round(layout.maximum)} aria-valuenow={Math.round(layout.history)}
          title="Drag to resize conversations. Double-click to reset."
          onkeydown={resizeKey}
          ondblclick={() => { historyOffset = 0; saveWidth(); }}
          onpointerdown={(event) => {
            if (event.button !== 0) return;
            event.preventDefault();
            event.currentTarget.focus();
            event.currentTarget.setPointerCapture(event.pointerId);
            resizing = true;
          }}
          onpointermove={(event) => {
            if (resizing && host) setWidth(event.clientX - host.getBoundingClientRect().left - 4);
          }}
          onpointerup={finishResize} onpointercancel={finishResize} onlostpointercapture={finishResize}
        ></div>
      {/if}
      {#if drawerOpen}
        <!-- Tapping the conversation is the obvious way to dismiss a drawer. -->
        <button
          type="button"
          class="scrim"
          aria-label="Close the chat list"
          onclick={() => (drawerOpen = false)}
        ></button>
      {/if}
      <div class="conversation-pane">
        <AssistantConversation {thread} active={active && ready} {onOpenSettings} {onNavigate} {onFindLinks} />
      </div>
    </div>
  {/if}
  {#if !ready && !error}<p role="status">Opening Chat...</p>{/if}
</div>

<style>
  .chat-view { display: flex; flex-direction: column; box-sizing: border-box; flex: 1; min-width: 0; min-height: 0; height: 100%; color: var(--text-primary); background: var(--bg-primary); }
  /* Positions the off-canvas chat list below the drawer breakpoint. */
  .conversation-host { position: relative; display: flex; flex: 1; min-height: 0; min-width: 0; }
  .conversation-host[hidden] { display: none; }
  .conversation-pane { display: flex; flex: 1; min-width: 0; min-height: 0; box-sizing: border-box; padding: 16px 16px 16px 8px; margin-right: var(--chat-reading-inset); }
  .resizing { user-select: none; cursor: col-resize; }
  .history-resizer { flex: 0 0 8px; cursor: col-resize; touch-action: none; position: relative; }
  .history-resizer::after { content: ""; position: absolute; inset: 0 3px; background: var(--border); }
  .history-resizer:hover::after, .history-resizer:focus-visible::after, .resizing .history-resizer::after { background: var(--accent); }
  .history-resizer:focus-visible { outline: 1px solid var(--accent); outline-offset: -1px; }
  p { margin: 0 0 8px; color: var(--danger, #c0392b); font-size: 13px; }
  /* The list sits beside the conversation on anything wide enough to hold both,
     so the toggle and its scrim only exist below the breakpoint. */
  .chat-bar { display: none; }
  .scrim { display: none; }
  .compact .chat-bar { display: flex; align-items: center; padding: 8px 10px 0; }
  .compact .conversation-pane { padding: 10px; margin-right: 0; }
  .chats-toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-height: 32px;
    padding: 4px 10px;
    border: 1px solid var(--border-color, #ddd);
    border-radius: 6px;
    background: var(--bg-secondary, #f5f5f5);
    color: var(--text-primary);
    font-size: 12px;
    cursor: pointer;
  }
  .compact .scrim {
    display: block;
    position: absolute;
    inset: 0;
    z-index: 10;
    width: 100%;
    padding: 0;
    border: 0;
    background: rgba(0, 0, 0, 0.38);
    cursor: pointer;
  }
</style>
