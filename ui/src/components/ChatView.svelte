<script lang="ts">
  import { untrack } from "svelte";
  import AssistantConversation from "./AssistantConversation.svelte";
  import { getGraphInfo } from "../lib/api";
  import {
    getGlobalConversation, getAssistantConversation,
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
  $effect(() => {
    const id = conversationId;
    if (!active) return;
    let disposed = false;
    ready = false;
    void getGraphInfo().then((graph) => {
      if (disposed) return;
      const requested = id ? getAssistantConversation(id) : undefined;
      if (id && !requested) throw new Error("This conversation is no longer available. Open Chat to start a general conversation.");
      if (requested && requested.graphPath !== graph.path) throw new Error("Return to the original graph to open this conversation.");
      const next = requested ?? getGlobalConversation(graph.path);
      if (untrack(() => thread?.id) !== next.id) thread = next;
      error = "";
      ready = true;
    }).catch((cause) => { if (!disposed) error = `Could not open Chat: ${String(cause)}`; });
    return () => { disposed = true; };
  });
</script>

<div class="chat-view">
  {#if error}<p role="alert">{error}</p>{/if}
  {#if thread}
    <div class="conversation-host" hidden={!ready} inert={!ready}>
      <AssistantConversation {thread} active={active && ready} {onOpenSettings} {onNavigate} {onFindLinks} />
    </div>
  {/if}
  {#if !ready && !error}<p role="status">Opening Chat...</p>{/if}
</div>

<style>
  .chat-view { display: flex; flex-direction: column; box-sizing: border-box; flex: 1; min-width: 0; min-height: 0; height: 100%; padding: 16px 24px; color: var(--text-primary); background: var(--bg-primary); }
  .conversation-host { display: flex; flex: 1; min-height: 0; min-width: 0; }
  .conversation-host[hidden] { display: none; }
  p { margin: 0 0 8px; color: var(--danger, #c0392b); font-size: 13px; }
  @media (max-width: 640px) { .chat-view { padding: 10px; } }
</style>
