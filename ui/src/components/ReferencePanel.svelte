<script lang="ts">
  import AssistantConversation from "./AssistantConversation.svelte";
  import ReadingNotesPanel from "./ReadingNotesPanel.svelte";
  import { getGraphInfo } from "../lib/api";
  import { assistantContextInfo } from "../lib/assistant";
  import { getSourceConversation, getAssistantConversation, type AssistantThread } from "../lib/assistantConversations";
  import { getLatestCurrentBlockAnchor, type CurrentBlockAnchor } from "../lib/currentBlockAnchor";
  import type { PageNavigationTarget } from "../lib/navigation";

  let {
    visible = false, pageId = "", pageTitle = "", conversationId = null, initialTab, focusTrigger = 0,
    noteFocusLabel = null, noteFocusPageId = null, noteFocusTrigger = 0,
    width = 380, preferFocusedPageForPageScope = false, onClose = () => {},
    onNavigate = () => {}, onFindLinks, onExpandConversation, onOpenSettings = () => onNavigate("__settings__"),
  }: {
    visible?: boolean; pageId?: string; pageTitle?: string; conversationId?: string | null;
    initialTab?: "chat" | "notes" | "references" | "ask" | "writing" | "search"; focusTrigger?: number;
    noteFocusLabel?: string | null; noteFocusPageId?: string | null; noteFocusTrigger?: number;
    width?: number; preferFocusedPageForPageScope?: boolean; onClose?: () => void;
    onNavigate?: (target: PageNavigationTarget) => void;
    onFindLinks?: (page: { id: string; title: string }, exactOnly?: boolean) => void;
    onExpandConversation?: (id: string) => void; onOpenSettings?: () => void;
  } = $props();

  let activeTab = $state<"chat" | "notes">("chat");
  let toolsRequested = $state(false);
  let askBlockAnchor = $state<CurrentBlockAnchor | null>(null);
  let thread = $state.raw<AssistantThread | null>(null);
  let sourceError = $state("");
  let sourceReady = $state(false);
  const requestedConversation = $derived(conversationId ? getAssistantConversation(conversationId) : undefined);
  const sourcePageId = $derived(requestedConversation?.sourcePageId ?? (preferFocusedPageForPageScope ? askBlockAnchor?.pageId ?? pageId : pageId));
  const sourceBlockId = $derived(askBlockAnchor?.pageId === sourcePageId ? askBlockAnchor.blockId : null);

  $effect(() => {
    focusTrigger;
    if (!initialTab) return;
    activeTab = initialTab === "notes" ? "notes" : "chat";
    toolsRequested = initialTab === "writing";
  });
  $effect(() => {
    pageId;
    askBlockAnchor = getLatestCurrentBlockAnchor();
    const changed = () => { askBlockAnchor = getLatestCurrentBlockAnchor(); };
    window.addEventListener("page-content-focus-changed", changed);
    return () => window.removeEventListener("page-content-focus-changed", changed);
  });
  $effect(() => {
    const id = sourcePageId;
    const requested = requestedConversation;
    const title = id === pageId ? pageTitle : "";
    if (!visible) return;
    let disposed = false;
    sourceError = "";
    sourceReady = false;
    if (!id && !requested) { thread = null; return; }
    void (async () => {
      try {
        const graph = await getGraphInfo();
        if (disposed) return;
        if (requested) {
          if (requested.graphPath !== graph.path) throw new Error("Return to the original graph to open this conversation.");
          thread = requested;
          sourceReady = true;
          return;
        }
        if (!id) { thread = null; return; }
        const info = await assistantContextInfo(graph.path, id);
        if (disposed) return;
        const isBookRoot = info.isBook && (!info.book || info.book.pageId === id);
        thread = getSourceConversation(graph.path, id, info.pageTitle || title, isBookRoot);
        sourceReady = true;
      } catch (cause) {
        if (disposed) return;
        sourceError = `Could not read source details: ${String(cause)}`;
      }
    })();
    return () => { disposed = true; };
  });
</script>

{#if visible}
  <aside class="reference-panel" style:width="{width}px" aria-label="Reading panel">
    <header class="panel-header">
      <div class="panel-tabs" role="tablist" aria-label="Reading panel tabs">
        <button role="tab" aria-selected={activeTab === "chat"} class:active={activeTab === "chat"} onclick={() => activeTab = "chat"}>Chat</button>
        <button role="tab" aria-selected={activeTab === "notes"} class:active={activeTab === "notes"} onclick={() => activeTab = "notes"}>Notes</button>
      </div>
      <button class="close-btn" onclick={onClose} aria-label="Close reading panel">×</button>
    </header>
    <div class="panel-content">
      {#if activeTab === "notes"}
        <ReadingNotesPanel pageId={sourcePageId} pageTitle={sourcePageId === pageId ? pageTitle : ""}
          active={visible && activeTab === "notes"} {onNavigate}
          initialNoteLabel={noteFocusLabel} initialNotePageId={noteFocusPageId} {noteFocusTrigger} />
      {/if}
      {#if thread}
        <!-- Keep source-bound tool previews alive across navigation and Notes. -->
        <div class="conversation-host" hidden={activeTab !== "chat" || !sourceReady} inert={activeTab !== "chat" || !sourceReady}>
          <AssistantConversation {thread} compact active={visible && activeTab === "chat" && sourceReady}
            blockId={sourceReady ? sourceBlockId : null} openTools={toolsRequested}
            {onNavigate} {onFindLinks} {onOpenSettings} onExpand={onExpandConversation} />
        </div>
      {/if}
      {#if activeTab === "chat"}
        {#if !thread && !sourcePageId}
          <p>Open a page, book, or journal day to start a conversation about it. For a general question, open full Chat.</p>
        {:else if !sourceReady && !sourceError}<p role="status">Opening this source’s conversation…</p>{/if}
        {#if sourceError}<p class="error" role="alert">{sourceError}</p>{/if}
      {/if}
    </div>
  </aside>
{/if}

<style>
  .reference-panel { position: fixed; top: 0; right: 0; bottom: 0; max-width: calc(100vw - 24px); background: var(--bg-secondary); border-left: 1px solid var(--border); display: flex; flex-direction: column; z-index: 1000; overflow: hidden; }
  .panel-header { display: flex; align-items: center; justify-content: space-between; padding: 8px 12px; border-bottom: 1px solid var(--border); flex-shrink: 0; }
  .panel-tabs { display: flex; flex-wrap: wrap; gap: 4px; }
  button { background: none; border: none; color: var(--text-secondary); padding: 6px 12px; border-radius: 4px; cursor: pointer; font: inherit; font-size: 13px; }
  button:hover { background: var(--bg-hover); color: var(--text-primary); }
  button.active { color: var(--text-primary); background: var(--bg-active, var(--bg-hover)); }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .close-btn { font-size: 20px; padding: 2px 8px; }
  .panel-content { flex: 1; min-height: 0; min-width: 0; overflow: hidden; padding: 12px; display: flex; flex-direction: column; }
  .conversation-host { display: flex; flex: 1; min-height: 0; min-width: 0; }
  .conversation-host[hidden] { display: none; }
  p { font-size: 13px; line-height: 1.5; color: var(--text-secondary); overflow-wrap: anywhere; }
  .error { color: var(--danger, #c0392b); }
</style>
