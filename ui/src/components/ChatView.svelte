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

  let { active = true, conversationId = null, onOpenSettings = () => {}, onNavigate = () => {}, onFindLinks }: {
    active?: boolean; conversationId?: string | null; onOpenSettings?: () => void;
    onNavigate?: (target: PageNavigationTarget) => void;
    onFindLinks?: (page: { id: string; title: string }, exactOnly?: boolean) => void;
  } = $props();
  let thread = $state.raw<AssistantThread | null>(null);
  let error = $state("");
  let ready = $state(false);
  let graphPath = $state("");
  // Only meaningful below the drawer breakpoint; above it the list is always
  // beside the conversation and CSS ignores this.
  let drawerOpen = $state(false);
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
      event.stopPropagation();
    }
  }}
/>

<div class="chat-view">
  {#if error}<p role="alert">{error}</p>{/if}
  {#if thread}
    <div class="chat-bar">
      <button
        type="button"
        class="chats-toggle"
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
    <div class="conversation-host" hidden={!ready} inert={!ready}>
      <ChatSwitcher
        {graphPath}
        currentId={thread.id}
        open={drawerOpen}
        onSelect={(next) => { thread = next; error = ""; drawerOpen = false; }}
      />
      {#if drawerOpen}
        <!-- Tapping the conversation is the obvious way to dismiss a drawer. -->
        <button
          type="button"
          class="scrim"
          aria-label="Close the chat list"
          onclick={() => (drawerOpen = false)}
        ></button>
      {/if}
      <AssistantConversation {thread} active={active && ready} {onOpenSettings} {onNavigate} {onFindLinks} />
    </div>
  {/if}
  {#if !ready && !error}<p role="status">Opening Chat...</p>{/if}
</div>

<style>
  .chat-view { display: flex; flex-direction: column; box-sizing: border-box; flex: 1; min-width: 0; min-height: 0; height: 100%; padding: 16px 24px; color: var(--text-primary); background: var(--bg-primary); }
  /* Positions the off-canvas chat list below the drawer breakpoint. */
  .conversation-host { position: relative; display: flex; flex: 1; min-height: 0; min-width: 0; }
  .conversation-host[hidden] { display: none; }
  p { margin: 0 0 8px; color: var(--danger, #c0392b); font-size: 13px; }
  /* The list sits beside the conversation on anything wide enough to hold both,
     so the toggle and its scrim only exist below the breakpoint. */
  .chat-bar { display: none; }
  .scrim { display: none; }
  @media (max-width: 640px) { .chat-view { padding: 10px; } }
  @media (max-width: 560px) {
    .chat-bar { display: flex; align-items: center; margin-bottom: 8px; }
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
    .scrim {
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
  }
</style>
