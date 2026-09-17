<script lang="ts">
  import { tick, untrack } from "svelte";
  import { open as openExternal } from "@tauri-apps/plugin-shell";
  import ChatMessageBubble from "./ChatMessageBubble.svelte";
  import ChatStatusTrail from "./ChatStatusTrail.svelte";
  import { isNearBottom, scrollToBottom } from "../lib/scrollToBottom";
  import { getGraphInfo } from "../lib/api";
  import { aiHealthCheck, type WebSource } from "../lib/knowledge";
  import { researchScopeInfo, type ResearchScope, type ResearchScopeInfo } from "../lib/research";
  import { readingSelection } from "../lib/readingSelection";
  import {
    getResearchThread, researchThreadChanges, updateResearchThread, researchThreadRunning,
    researchTarget, sendResearchQuestion, stopResearchThread, newResearchConversation, type ResearchThread,
  } from "../lib/researchThreads";
  import { initialState, statusDisplay, statusTrail, finishedTrail } from "../lib/chatStatus";
  import type { ChatThinkingTone } from "../lib/chatMessage";
  import type { PageNavigationTarget } from "../lib/navigation";

  let {
    pageId, pageTitle, blockId = null, active = true,
    onNavigate = () => {}, onOpenSettings = () => {},
  }: {
    pageId: string; pageTitle: string; blockId?: string | null; active?: boolean;
    onNavigate?: (target: PageNavigationTarget) => void; onOpenSettings?: () => void;
  } = $props();

  let thread = $state.raw<ResearchThread | null>(null);
  let info = $state<ResearchScopeInfo | null>(null);
  let sourceError = $state<string | null>(null);
  let connectionError = $state<string | null>(null);
  let connected = $state(false);
  let checking = $state(true);
  let now = $state(Date.now());
  let reducedMotion = $state(false);
  let scrollEl: HTMLDivElement | undefined;
  let followAnswer = $state(true);
  const view = $derived.by(() => {
    $researchThreadChanges;
    return thread ? { ...thread, messages: thread.messages.map((message) => ({ ...message })) } : null;
  });
  const running = $derived(!!view && researchThreadRunning(view));
  const status = $derived(statusDisplay(view?.state ?? initialState(), now, reducedMotion));
  const trail = $derived(statusTrail(view?.state ?? initialState(), now, reducedMotion));
  const pageLabel = $derived(info?.isJournal ? "This journal day" : info?.isBook ? "This book" : "This page");
  const scopeUnavailable = $derived(!view || (view.scope === "selection" && (!view.selection || !!view.selectionError))
    || (view.scope === "block" && !blockId) || (view.scope === "section" && !info?.section));

  $effect(() => {
    if (!active) return;
    let disposed = false;
    checking = true;
    connectionError = null;
    aiHealthCheck().then((health) => {
      if (!disposed) connected = health.enabled && health.llm_available;
    }).catch((error) => {
      if (!disposed) { connected = false; connectionError = `Could not check the AI connection: ${String(error)}`; }
    }).finally(() => { if (!disposed) checking = false; });
    return () => { disposed = true; };
  });

  $effect(() => {
    const id = pageId;
    const title = pageTitle;
    const anchor = blockId;
    if (!active) return;
    let disposed = false;
    info = null;
    sourceError = null;
    if (!id) { thread = null; return; }
    if (untrack(() => thread?.pageId) !== id) thread = null;
    void (async () => {
      try {
        const graph = await getGraphInfo();
        if (disposed) return;
        thread = getResearchThread(graph.path, id, title);
        updateResearchThread();
        const result = await researchScopeInfo(graph.path, id, anchor ?? undefined);
        if (disposed) return;
        if (result.pageId !== id) throw new Error("The source information no longer matches this page.");
        info = result;
        thread.pageTitle = result.pageTitle;
        updateResearchThread();
      } catch (error) {
        if (!disposed) sourceError = `Could not read source scope: ${String(error)}`;
      }
    })();
    return () => { disposed = true; };
  });

  $effect(() => {
    const captured = $readingSelection;
    const destination = thread;
    if (!destination || !captured.pageIds.includes(destination.pageId)) return;
    if (captured.error) {
      destination.selection = null;
      destination.selectionError = captured.error;
    } else if (captured.selection?.pageId === destination.pageId) {
      destination.selection = captured.selection;
      destination.selectionError = null;
    } else return;
    updateResearchThread();
  });

  $effect(() => {
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    reducedMotion = mq.matches;
    const changed = () => { reducedMotion = mq.matches; };
    mq.addEventListener("change", changed);
    return () => mq.removeEventListener("change", changed);
  });

  $effect(() => {
    if (!active || !running) return;
    now = Date.now();
    const clock = setInterval(() => { now = Date.now(); }, 1000);
    return () => clearInterval(clock);
  });

  $effect(() => {
    const messages = view?.messages;
    // New step rows grow the transcript the same way new messages do.
    trail.rows.length;
    if (!active || !messages || !followAnswer) return;
    void tick().then(() => {
      if (active) scrollToBottom(scrollEl);
    });
  });

  function changeScope(value: string) {
    if (!thread || running) return;
    thread.scope = value as ResearchScope;
    updateResearchThread();
  }

  function clearSelection() {
    if (!thread) return;
    thread.selection = null;
    thread.selectionError = null;
    if (thread.scope === "selection") thread.scope = "page";
    readingSelection.set({ selection: null, error: null, pageIds: [] });
    updateResearchThread();
  }

  async function send() {
    const destination = thread;
    if (!destination || running || checking || !connected || scopeUnavailable) return;
    try {
      // All scope values are captured before saving editors or crossing any await.
      const target = researchTarget(destination.pageId, destination.scope, blockId, info?.section?.blockId ?? null, destination.selection);
      followAnswer = true;
      await sendResearchQuestion(destination, target);
    } catch (error) {
      destination.error = String(error);
      updateResearchThread();
    }
  }

  async function openWebSource(source: WebSource) {
    const destination = thread;
    try {
      const url = new URL(source.url);
      if (!["https:", "http:"].includes(url.protocol)) throw new Error("Only HTTP and HTTPS source links can be opened.");
      await openExternal(url.href);
    } catch (error) {
      if (destination) { destination.error = `Could not open source: ${String(error)}`; updateResearchThread(); }
    }
  }

  const thinkingTone = $derived<ChatThinkingTone>(status.kind === "stalled" ? "stalled"
    : status.phase === "searching_web" || status.phase === "reading_sources" ? "web"
    : status.phase === "thinking" ? "thinking" : "working");
</script>

<section class="research-conversation" aria-label="Research conversation">
  <header class="conversation-header">
    <div class="source-heading">
      <strong title={view?.pageTitle || pageTitle}>{view?.pageTitle || pageTitle || "No source page"}</strong>
      <span>Conversation stays with this source</span>
    </div>
    <button type="button" class="quiet-button" disabled={!view || running || !view.messages.length}
      onclick={() => { if (thread) newResearchConversation(thread); }}>New conversation</button>
  </header>

  <div class="conversation-scroll" bind:this={scrollEl}
    onscroll={() => { followAnswer = isNearBottom(scrollEl); }}>
    {#if !pageId}
      <p class="empty-message">Open a page, book, or journal day to ask about what you’re reading.</p>
    {:else if !view?.messages.length}
      <div class="empty-message">
        <p>Ask about this source</p>
        <span>Explain an idea, compare passages, or ask a follow-up. Select text in your notes to focus on a passage.</span>
      </div>
    {/if}
    {#each view?.messages ?? [] as message, index}
      {#if message.role === "assistant"}
        {#if view?.pendingIndex === index}
          <ChatStatusTrail {trail} note={view?.note ?? ""} meta={status.meta}
            notice={status.kind === "stalled" || status.kind === "error" ? status.label : ""} />
        {:else if message.steps?.length}
          <ChatStatusTrail trail={finishedTrail(message.steps)} collapsed />
        {/if}
      {/if}
      <ChatMessageBubble {message} {index} streaming={running && view?.pendingIndex === index}
        animateCursor={status.animate} thinkingLabel={status.announce} {thinkingTone}
        trailed={view?.pendingIndex === index && trail.any}
        onOpenSource={(source) => onNavigate({ id: source.page_id })}
        onOpenWebSource={openWebSource} />
    {/each}
    {#if view?.error}<p class="error-message" role="alert">{view.error}</p>{/if}
    {#if view?.state.kind === "cancelled"}<p class="status-message" role="status">Stopped. Any partial answer is kept above.</p>{/if}
  </div>

  <div class="conversation-controls">
    {#if sourceError}<p class="error-message" role="alert">{sourceError}</p>{/if}
    {#if connectionError}<p class="error-message" role="alert">{connectionError}</p>{/if}
    {#if !checking && !connected}
      <p class="connection-notice">Connect an AI provider to ask questions. <button type="button" class="text-button" onclick={onOpenSettings}>Configure in Settings</button></p>
    {/if}
    {#if view?.selectionError}<p class="error-message" role="alert">{view.selectionError}</p>{/if}
    {#if view?.selection}
      <div class="selection-preview">
        <div><span>Selected passage · {view.selection.blockIds.length} block{view.selection.blockIds.length === 1 ? "" : "s"}</span>
          <button type="button" class="text-button" disabled={running} onclick={clearSelection}>Clear</button></div>
        <blockquote>{view.selection.text}</blockquote>
      </div>
    {/if}
    <form class="conversation-composer" onsubmit={(event) => { event.preventDefault(); void send(); }}>
      <textarea aria-label="Research question" placeholder="Ask about this source…" rows="3"
        value={view?.draft ?? ""} disabled={!view || !connected || checking}
        oninput={(event) => { if (thread) { thread.draft = event.currentTarget.value; updateResearchThread(); } }}
        onkeydown={(event) => {
          if (event.key === "Enter" && !event.shiftKey && !event.ctrlKey && !event.altKey && !event.metaKey && !event.isComposing) {
            event.preventDefault(); void send();
          }
        }}></textarea>
      <div class="composer-options">
        <label class="scope-picker">Scope
          <select aria-label="Research scope" value={view?.scope ?? "page"} disabled={!view || running}
            onchange={(event) => changeScope(event.currentTarget.value)}>
            <option value="page">{pageLabel}</option>
            <option value="selection" disabled={!view?.selection || !!view?.selectionError}>Selection</option>
            <option value="block" disabled={!blockId}>Current block</option>
            <option value="section" disabled={!info?.section}>Current section</option>
          </select>
        </label>
        <div class="web-options">
          <label><input type="checkbox" checked={view?.internet ?? false} disabled={!view || running}
            onchange={(event) => {
              if (thread) { thread.internet = event.currentTarget.checked; if (!thread.internet) thread.research = false; updateResearchThread(); }
            }} />Internet</label>
          <label title="Opt into multi-step searches and source reading">
            <input type="checkbox" checked={view?.research ?? false} disabled={!view?.internet || running}
              onchange={(event) => { if (thread) { thread.research = event.currentTarget.checked; updateResearchThread(); } }} />Research
          </label>
        </div>
        {#if running}
          <button type="button" class="send-button stop-button" onclick={() => { if (thread) void stopResearchThread(thread); }}>Stop</button>
        {:else}
          <button type="submit" class="send-button" disabled={!view?.draft.trim() || scopeUnavailable || !connected || checking}>Send</button>
        {/if}
      </div>
    </form>
    {#if view?.scope === "section" && info?.section}<p class="scope-detail">Section: {info.section.title}</p>{/if}
    {#if view?.scope === "block" && blockId}<p class="scope-detail">Current block and its children</p>{/if}
    {#if scopeUnavailable && view?.scope !== "page"}<p class="error-message">This scope is no longer available. Select it again in your notes or choose {pageLabel.toLowerCase()}.</p>{/if}
    <p class="privacy-note">A cloud provider receives your question and selected source excerpts. Internet sends derived queries to search engines and reads websites. {view?.internet ? view.research ? "Multi-step research is on." : "Web search is on." : "Internet is off."} Answers never change your notes.</p>
  </div>
</section>

<style>
  .research-conversation { display: flex; flex-direction: column; flex: 1; min-width: 0; min-height: 0; gap: 10px; color: var(--text-primary); font-size: 13px; }
  .conversation-header { display: flex; flex-wrap: wrap; align-items: start; justify-content: space-between; gap: 8px; flex-shrink: 0; }
  .source-heading { min-width: 0; flex: 1 1 140px; display: grid; gap: 3px; }
  .source-heading strong { font-size: 14px; overflow-wrap: anywhere; }
  .source-heading span, .scope-detail, .privacy-note, .status-message { color: var(--text-secondary); font-size: 12px; }
  button, textarea, select { font: inherit; }
  button { cursor: pointer; border-radius: 5px; }
  button:disabled, select:disabled { cursor: default; opacity: .55; }
  button:focus-visible, select:focus-visible, textarea:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .quiet-button { color: var(--text-secondary); background: var(--bg-primary); border: 1px solid var(--border); padding: 5px 8px; }
  .quiet-button:hover:not(:disabled) { background: var(--bg-hover); color: var(--text-primary); }
  .conversation-scroll { flex: 1; min-height: 64px; overflow-y: auto; overflow-x: hidden; display: flex; flex-direction: column; gap: 10px; overscroll-behavior: contain; padding-bottom: 14px; }
  .conversation-scroll :global(.msg) { min-width: 0; flex-shrink: 0; overflow-wrap: anywhere; }
  .conversation-scroll :global(.msg pre), .conversation-scroll :global(.msg table) { max-width: 100%; overflow-x: auto; }
  .empty-message { padding: 16px 2px; line-height: 1.55; color: var(--text-secondary); }
  .empty-message p { color: var(--text-primary); font-weight: 600; margin: 0 0 6px; }
  .conversation-controls { flex-shrink: 0; display: flex; flex-direction: column; gap: 8px; max-height: 65%; overflow-y: auto; min-width: 0; }
  .conversation-composer { border: 1px solid var(--border); background: var(--bg-primary); border-radius: 7px; min-width: 0; }
  .conversation-composer:focus-within { border-color: var(--accent); }
  textarea { display: block; box-sizing: border-box; width: 100%; min-height: 68px; max-height: 180px; resize: vertical; border: none; padding: 10px; color: var(--text-primary); background: transparent; line-height: 1.5; }
  textarea::placeholder { color: var(--text-secondary); }
  .composer-options { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; padding: 6px 8px 8px; }
  .scope-picker { display: flex; align-items: center; flex: 1 1 150px; min-width: 0; gap: 6px; color: var(--text-secondary); }
  select { min-width: 0; max-width: 100%; flex: 1; color: var(--text-primary); background: var(--bg-secondary); border: 1px solid var(--border); padding: 4px; border-radius: 4px; }
  .web-options { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; }
  .web-options label { display: flex; gap: 4px; align-items: center; white-space: nowrap; }
  input { accent-color: var(--accent); margin: 0; }
  .send-button { border: 1px solid transparent; padding: 5px 12px; margin-left: auto; color: var(--btn-primary-fg, var(--bg-primary)); background: var(--btn-primary-bg, var(--accent)); }
  .send-button:hover:not(:disabled) { filter: brightness(1.1); }
  .stop-button { background: var(--bg-secondary); color: var(--text-primary); border-color: var(--border); }
  .error-message { color: var(--danger, #c0392b); overflow-wrap: anywhere; margin: 0; font-size: 12px; line-height: 1.5; }
  .connection-notice, .scope-detail, .privacy-note, .status-message { margin: 0; line-height: 1.5; overflow-wrap: anywhere; }
  .text-button { border: none; background: transparent; color: var(--accent); padding: 0; text-decoration: underline; }
  .selection-preview { border: 1px solid var(--border); padding: 6px 8px; border-radius: 5px; min-width: 0; }
  .selection-preview > div { display: flex; justify-content: space-between; gap: 8px; font-size: 12px; color: var(--text-secondary); }
  blockquote { margin: 6px 0 0; max-height: 72px; overflow-y: auto; white-space: pre-wrap; overflow-wrap: anywhere; line-height: 1.45; }
</style>
