<script lang="ts">
  import { tick, untrack } from "svelte";
  import { open as openExternal } from "@tauri-apps/plugin-shell";
  import ChatMessageBubble from "./ChatMessageBubble.svelte";
  import AssistantDiagnostics from "./AssistantDiagnostics.svelte";
  import PageAssistantTools from "./PageAssistantTools.svelte";
  import AIEditPlanCard from "./AIEditPlanCard.svelte";
  import ChatStatusTrail from "./ChatStatusTrail.svelte";
  import { assistantModes, assistantProvider } from "./assistantPresentation";
  import { listBlocks } from "../lib/api";
  import { aiAsk, aiHealthCheck, aiGetConfig, type AiConfig, type WebSource } from "../lib/knowledge";
  import { buildPlannerPrompt, hydratePlan, looksLikeEditRequest, parseEditPlan, type EditAction } from "../lib/aiActions";
  import { applyEditPlan, summarizeApplyResult, type BlockTarget } from "../lib/aiActionsApply";
  import { captureResearchSource } from "../lib/researchSource";
  import { runUndoOperation } from "../lib/undoStack";
  import { flushPageEditors, withPageEditorsLocked } from "../lib/editorPersistence";
  import { assistantContextInfo, type AssistantContext, type AssistantContextInfo, type AssistantMode } from "../lib/assistant";
  import {
    assistantConversationChanges, updateAssistantConversation, assistantConversationRunning,
    sendAssistantQuestion, stopAssistantConversation, newAssistantConversation,
    type AssistantThread,
  } from "../lib/assistantConversations";
  import { readingSelection } from "../lib/readingSelection";
  import { getLatestCurrentBlockAnchor, type CurrentBlockAnchor } from "../lib/currentBlockAnchor";
  import { selectionIntersectsTranscript } from "../lib/transcriptSelection";
  import { initialState, statusDisplay, statusTrail, finishedTrail } from "../lib/chatStatus";
  import type { ChatThinkingTone } from "../lib/chatMessage";
  import type { PageNavigationTarget } from "../lib/navigation";

  let { thread, active = true, compact = false, blockId = null, openTools = false,
    onNavigate = () => {}, onOpenSettings = () => {}, onExpand, onFindLinks,
  }: {
    thread: AssistantThread; active?: boolean; compact?: boolean; blockId?: string | null; openTools?: boolean;
    onNavigate?: (target: PageNavigationTarget) => void; onOpenSettings?: () => void;
    onExpand?: (id: string) => void;
    onFindLinks?: (page: { id: string; title: string }, exactOnly?: boolean) => void;
  } = $props();

  let info = $state<AssistantContextInfo | null>(null);
  let sourceError = $state("");
  let connected = $state(false);
  let checking = $state(true);
  let connectionError = $state("");
  let config = $state<AiConfig | null>(null);
  let anchor = $state<CurrentBlockAnchor | null>(null);
  let now = $state(Date.now());
  let reducedMotion = $state(false);
  let scrollEl: HTMLDivElement | undefined;
  let inputEl: HTMLTextAreaElement | undefined;
  let paneEl: HTMLElement | undefined;
  let footerEl: HTMLDivElement | undefined;
  let followAnswer = $state(true);
  let blockPreview = $state("");
  let planning = $state(false);
  let applyingPlan = $state(false);
  let planError = $state("");
  let planResult = $state("");
  let planLinks = $state<{ id: string; title: string }[]>([]);
  let pendingPlan = $state<{ request: string; actions: EditAction[] } | null>(null);
  let pointerDown = false;
  let refocusPending = false;
  const view = $derived.by(() => {
    $assistantConversationChanges;
    return { ...thread, messages: thread.messages.map((message) => ({ ...message })) };
  });
  const running = $derived(assistantConversationRunning(view));
  const status = $derived(statusDisplay(view.state ?? initialState(), now, reducedMotion));
  const trail = $derived(statusTrail(view.state ?? initialState(), now, reducedMotion));
  const provider = $derived(assistantProvider(config));
  const contextPageId = $derived(thread.sourcePageId ?? ("pageId" in view.context ? view.context.pageId : null));
  const focusedBlockId = $derived(blockId ?? (anchor?.pageId === contextPageId ? anchor?.blockId : null));
  const pageLabel = $derived(info?.isJournal ? "This day" : "This page");
  const selection = $derived(view.context.kind === "selection" ? view.context.selection : null);
  const scopeUnavailable = $derived(
    (view.context.kind === "selection" && (!view.context.selection.blockIds.length || !view.context.selection.text.trim()))
    || (["block", "section"].includes(view.context.kind) && !("blockId" in view.context && view.context.blockId))
  );
  const thinkingTone = $derived<ChatThinkingTone>(status.kind === "stalled" ? "stalled"
    : status.phase === "searching_web" || status.phase === "reading_sources" ? "web"
    : status.phase === "thinking" ? "thinking" : "working");

  $effect(() => {
    if (!active) return;
    let disposed = false;
    async function refresh() {
      checking = true;
      const [healthResult, configResult] = await Promise.allSettled([aiHealthCheck(), aiGetConfig()]);
      if (disposed) return;
      connected = healthResult.status === "fulfilled" && healthResult.value.enabled && healthResult.value.llm_available;
      connectionError = healthResult.status === "rejected" ? `Could not check the model: ${String(healthResult.reason)}` : "";
      config = configResult.status === "fulfilled" ? configResult.value : null;
      checking = false;
    }
    void refresh();
    window.addEventListener("ai-configuration-changed", refresh);
    return () => { disposed = true; window.removeEventListener("ai-configuration-changed", refresh); };
  });

  $effect(() => {
    anchor = getLatestCurrentBlockAnchor();
    const changed = () => { anchor = getLatestCurrentBlockAnchor(); };
    window.addEventListener("page-content-focus-changed", changed);
    return () => window.removeEventListener("page-content-focus-changed", changed);
  });

  $effect(() => {
    const id = contextPageId;
    const graphPath = thread.graphPath;
    const block = focusedBlockId;
    if (!active) return;
    let disposed = false;
    info = null; sourceError = "";
    if (!id) return;
    void assistantContextInfo(graphPath, id, block ?? undefined).then((result) => {
      if (disposed) return;
      if (result.pageId !== id) throw new Error("Source page changed. Select the context again.");
      info = result;
    }).catch((cause) => { if (!disposed) sourceError = `Could not read context: ${String(cause)}`; });
    return () => { disposed = true; };
  });

  $effect(() => {
    const context = view.context;
    blockPreview = "";
    if (context.kind !== "block" && context.kind !== "section") return;
    let disposed = false;
    void listBlocks(context.pageId).then((blocks) => {
      if (!disposed) blockPreview = blocks.find((block) => block.id === context.blockId)?.content ?? "This block is no longer available. Choose context again.";
    }).catch(() => { if (!disposed) blockPreview = "Could not preview this block."; });
    return () => { disposed = true; };
  });

  $effect(() => {
    const captured = $readingSelection;
    const destination = thread;
    if (untrack(() => assistantConversationRunning(destination))) return;
    if (!contextPageId || !captured.pageIds.includes(contextPageId)) return;
    destination.selection = captured.selection?.pageId === contextPageId ? captured.selection : null;
    destination.selectionError = captured.error;
    updateAssistantConversation();
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
    view.messages;
    // A new step row grows the transcript just like a new message does, so the
    // trail has to drive the same follow-scroll or the live step slides out of
    // view exactly when it matters.
    trail.rows.length;
    if (!active || !followAnswer) return;
    void tick().then(() => {
      if (active && scrollEl?.isConnected && !window.getSelection()?.toString()) scrollEl.scrollTop = scrollEl.scrollHeight;
    });
  });

  function resizeComposer() {
    if (!inputEl || !paneEl) return;
    const maximum = Math.max(32, Math.floor(paneEl.clientHeight / 2) - (footerEl?.offsetHeight ?? 64) - 2);
    inputEl.style.height = "0px";
    inputEl.style.height = `${Math.min(maximum, Math.max(48, inputEl.scrollHeight))}px`;
    inputEl.style.overflowY = inputEl.scrollHeight > maximum ? "auto" : "hidden";
  }
  $effect(() => {
    view.draft;
    active;
    void tick().then(resizeComposer);
  });
  $effect(() => {
    thread.id;
    if (!active || compact) return;
    void tick().then(() => { if (active && inputEl?.isConnected) inputEl.focus(); });
  });
  function restoreInputFocus(afterRun = false) {
    if (!active || compact || running || !inputEl?.isConnected || inputEl.disabled) return;
    if (pointerDown) { refocusPending = true; return; }
    if (selectionIntersectsTranscript(window.getSelection(), scrollEl ?? null)) return;
    const focused = document.activeElement;
    if (focused instanceof Element && focused !== inputEl
      && !(afterRun && focused.matches(".send-button"))
      && focused.closest("input, textarea, select, button, a, summary, [contenteditable=true]")) return;
    inputEl.focus();
  }
  function onInputBlur() {
    if (pointerDown) refocusPending = true;
    else requestAnimationFrame(() => restoreInputFocus());
  }
  $effect(() => {
    if (!active || compact) return;
    const down = () => { pointerDown = true; };
    const up = () => {
      pointerDown = false;
      if (refocusPending) { refocusPending = false; requestAnimationFrame(() => restoreInputFocus()); }
    };
    document.addEventListener("mousedown", down);
    document.addEventListener("mouseup", up);
    return () => {
      document.removeEventListener("mousedown", down);
      document.removeEventListener("mouseup", up);
      pointerDown = false; refocusPending = false;
    };
  });
  $effect(() => {
    if (!active || compact || running) return;
    void tick().then(() => restoreInputFocus(true));
  });
  $effect(() => {
    if (!paneEl) return;
    const observer = new ResizeObserver(resizeComposer);
    observer.observe(paneEl);
    if (footerEl) observer.observe(footerEl);
    return () => observer.disconnect();
  });

  function chooseContext(kind: string) {
    if (running) return;
    const pageId = contextPageId;
    let context: AssistantContext;
    let label = "";
    const title = info?.pageTitle || thread.sourcePageTitle || "Source";
    if (kind === "none") { context = { kind: "none" }; label = "No notes"; }
    else if (kind === "graph") { context = { kind: "graph" }; label = "My graph"; }
    else if (kind === "page" && pageId) { context = { kind: "page", pageId }; label = `${pageLabel} · ${title}`; }
    else if (kind === "book" && info?.book) { context = { kind: "book", pageId: info.book.pageId }; label = `Whole book · ${info.book.title}`; }
    else if (kind === "book" && info?.isBook && pageId) { context = { kind: "book", pageId }; label = `Whole book · ${title}`; }
    else if (kind === "block" && focusedBlockId && pageId) {
      context = { kind: "block", pageId, blockId: focusedBlockId }; label = `Block including children · ${title}`;
    } else if (kind === "section" && info?.section && pageId) {
      context = { kind: "section", pageId, blockId: info.section.blockId }; label = `Section / Chapter · ${info.section.title} · ${title}`;
    } else if (kind === "selection" && view.selection && !view.selectionError) {
      context = { kind: "selection", pageId: view.selection.pageId, selection: { blockIds: [...view.selection.blockIds], text: view.selection.text } };
      label = `Selection · ${title}`;
    } else return;
    thread.context = context;
    thread.contextLabel = label;
    updateAssistantConversation();
  }

  async function send() {
    if (running || checking || !connected || scopeUnavailable || !thread.draft.trim()) return;
    const request = thread.draft.trim();
    followAnswer = true;
    inputEl?.focus();
    // An instruction ("add that to my journal") should change notes, not produce
    // another paragraph of prose. Questions skip this entirely so ordinary chat
    // keeps its current latency.
    if (looksLikeEditRequest(request) && (await proposeEdits(request))) return;
    await sendAssistantQuestion(thread, thread.context, thread.contextLabel);
  }

  function lastAnswer(): string {
    return [...view.messages].reverse().find((message) => message.role === "assistant")?.content ?? "";
  }

  /**
   * Ask the model for an edit plan. Returns true when a card is showing.
   *
   * Any failure returns false so `send` falls through to a normal answer: the
   * user asked for something, and a broken planner should never mean silence.
   */
  async function proposeEdits(request: string): Promise<boolean> {
    planning = true;
    planError = "";
    try {
      const answer = lastAnswer();
      const prompt = buildPlannerPrompt(request, answer, !!focusedBlockId);
      const response = await aiAsk(prompt, undefined, [],
        contextPageId ? { pageId: contextPageId, blockId: focusedBlockId ?? undefined } : undefined);
      const plan = hydratePlan(parseEditPlan(response.answer), answer);
      if (!plan.actions.length) return false;
      pendingPlan = { request, actions: plan.actions };
      thread.draft = "";
      updateAssistantConversation();
      return true;
    } catch (cause) {
      console.error("Could not plan edits:", cause);
      return false;
    } finally {
      planning = false;
    }
  }

  function dismissPlan() {
    // Hand the request back so a near-miss can be rephrased instead of retyped.
    if (pendingPlan && !thread.draft.trim()) {
      thread.draft = pendingPlan.request;
      updateAssistantConversation();
    }
    pendingPlan = null;
    planError = "";
    inputEl?.focus();
  }

  async function captureBlockTarget(actions: EditAction[]): Promise<BlockTarget | null> {
    if (!actions.some((action) => action.type === "replace_block")) return null;
    if (!contextPageId || !focusedBlockId) throw new Error("This conversation is not attached to a block.");
    const source = await captureResearchSource(contextPageId, focusedBlockId);
    const target = source.snapshot.find((block) => block.id === focusedBlockId);
    if (!target) throw new Error("The block this conversation was about is no longer there.");
    return {
      graphPath: source.graphPath,
      pageId: source.pageId,
      blockId: focusedBlockId,
      content: target.content,
      snapshot: source.snapshot,
    };
  }

  async function applyPlan() {
    const plan = pendingPlan;
    if (!plan || applyingPlan) return;
    applyingPlan = true;
    planError = "";
    try {
      const actions = plan.actions.map((action) => ({ ...action, tags: [...action.tags] }));
      const blockTarget = await captureBlockTarget(actions);
      const run = () => applyEditPlan({ actions }, { blockTarget });
      // Only a block rewrite races an open editor; appends to other pages do not.
      const result = await runUndoOperation(() =>
        blockTarget
          ? withPageEditorsLocked(blockTarget.pageId, async () => {
              await flushPageEditors(blockTarget.pageId);
              return run();
            })
          : run()
      );

      for (const touched of new Set(result.applied.map((entry) => entry.pageId))) {
        window.dispatchEvent(new CustomEvent("page-content-reload-blocks", { detail: { pageId: touched } }));
      }
      for (const target of result.findLinks) onFindLinks?.({ id: target.pageId, title: target.title });

      if (result.applied.length) {
        planResult = summarizeApplyResult(result);
        planLinks = result.applied
          .filter((entry) => entry.pageTitle)
          .map((entry) => ({ id: entry.pageId, title: entry.pageTitle }));
        pendingPlan = null;
      } else {
        planError = result.errors.join("; ") || "Nothing was applied.";
      }
    } catch (cause) {
      planError = `Could not apply the changes: ${cause instanceof Error ? cause.message : String(cause)}`;
    } finally {
      applyingPlan = false;
    }
  }

  function shortcut(question: string) {
    if (running) return;
    thread.draft = question;
    updateAssistantConversation();
    inputEl?.focus();
  }
  async function openWebSource(source: WebSource) {
    try {
      const url = new URL(source.url);
      if (!["https:", "http:"].includes(url.protocol)) throw new Error("Only HTTP and HTTPS sources can be opened.");
      await openExternal(url.href);
    } catch (cause) { thread.error = `Could not open source: ${String(cause)}`; updateAssistantConversation(); }
  }
</script>

<section class="assistant-conversation" class:compact aria-label="Chat conversation" bind:this={paneEl}>
  <header class="conversation-header">
    <div class="source-heading">
      <strong>{view.sourcePageTitle || "Chat"}</strong>
      <button class="model-badge" onclick={onOpenSettings} title={provider.detail}>{provider.label} <span>· {provider.detail}</span></button>
    </div>
    <div class="header-actions">
      {#if compact && onExpand}<button class="quiet-button" onclick={() => onExpand?.(thread.id)} title="Open this same conversation in full Chat">Expand</button>{/if}
      <button class="quiet-button" disabled={running || (!view.messages.length && !view.draft)}
        onclick={() => { newAssistantConversation(thread); inputEl?.focus(); }}>New conversation</button>
    </div>
  </header>

  <div class="conversation-scroll chat-log" bind:this={scrollEl}
    onscroll={() => { if (scrollEl) followAnswer = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight < 48; }}>
    {#if contextPageId}
      <PageAssistantTools pageId={contextPageId} pageTitle={info?.pageTitle || view.sourcePageTitle}
        blockId={focusedBlockId} {thread} {openTools} active={active} {onFindLinks} {onNavigate} {onOpenSettings} />
    {/if}
    <AssistantDiagnostics {active} {running} {onOpenSettings} />
    {#if !view.messages.length}
      <div class="empty-message">
        <p>{view.sourcePageId ? "Ask about what you’re reading" : "What would you like to explore?"}</p>
        <span>Choose your notes context and a mode. Answers never change your notes.</span>
        <div class="shortcuts">
          <button class="quiet-button" onclick={() => shortcut("Summarize the main ideas in the chosen context.")}>Summary</button>
          <button class="quiet-button" onclick={() => shortcut("Explain the main idea in the chosen context, with an example.")}>Explain</button>
          <button class="quiet-button" onclick={() => shortcut("Compare the ideas in the chosen context: ")}>Compare</button>
        </div>
      </div>
    {/if}
    {#each view.messages as message, index}
      <div class="conversation-turn">
        {#if message.role === "assistant" && (message.contextLabel || message.mode)}
          <div class="answer-badges"><span>{message.contextLabel || "Context recorded for this answer"}</span><span>{assistantModes[message.mode ?? "answer"].label}</span></div>
        {/if}
        {#if message.role === "assistant"}
          {#if view.pendingIndex === index}
            <ChatStatusTrail {trail} note={view.note} meta={status.meta}
              notice={status.kind === "stalled" || status.kind === "error" ? status.label : ""} />
          {:else if message.steps?.length}
            <ChatStatusTrail trail={finishedTrail(message.steps)} collapsed />
          {/if}
        {/if}
        <ChatMessageBubble {message} {index} streaming={running && view.pendingIndex === index}
          animateCursor={status.animate} thinkingLabel={status.announce} {thinkingTone}
          trailed={view.pendingIndex === index && trail.any}
          onOpenSource={(source) => window.dispatchEvent(new CustomEvent("navigate-page", { detail: { pageName: source.page_title, targetBlockId: source.block_id } }))}
          onOpenWebSource={openWebSource} />
      </div>
    {/each}
    {#if view.error}<p class="error-message" role="alert">{view.error}</p>{/if}
    {#if planning}<p class="status-message" role="status">Working out what to change…</p>{/if}
    {#if pendingPlan}
      <AIEditPlanCard request={pendingPlan.request} actions={pendingPlan.actions}
        applying={applyingPlan} error={planError} onApply={applyPlan} onDismiss={dismissPlan} />
    {:else if planError}
      <p class="error-message" role="alert">{planError}</p>
    {/if}
    {#if planResult}
      <p class="status-message" role="status">
        {planResult}
        {#each planLinks as link}
          <button class="text-button" onclick={() => onNavigate({ id: link.id, title: link.title })}>Open {link.title}</button>
        {/each}
        <button class="text-button" onclick={() => { planResult = ""; planLinks = []; }}>Dismiss</button>
      </p>
    {/if}
    {#if view.state.kind === "cancelled"}<p class="status-message" role="status">Stopped. Any partial answer is kept above.</p>{/if}
    {#if sourceError}<p class="error-message" role="alert">{sourceError}</p>{/if}
    {#if connectionError}<p class="error-message" role="alert">{connectionError}</p>{/if}
    {#if !checking && !connected}
      <p class="connection-notice">Connect a model to send questions. <button class="text-button" onclick={onOpenSettings}>Configure in Settings</button>. Drafts and manual Notes remain available.</p>
    {/if}
    {#if view.selectionError}<p class="error-message" role="alert">{view.selectionError}</p>{/if}
  </div>

  <div class="conversation-controls">
    <div class="context-preview">
      <span title={view.contextLabel}>{running ? "Answering with" : "Context"}: {view.contextLabel}</span>
      {#if view.selection && !view.selectionError && !running}
        <button class="text-button" onmousedown={(event) => event.preventDefault()} onclick={() => chooseContext("selection")}>Use selection</button>
      {/if}
    </div>
    {#if !running && view.context.kind === "block" && focusedBlockId && view.context.blockId !== focusedBlockId}
      <button class="text-button refresh-context" onclick={() => chooseContext("block")}>Use the newly focused block</button>
    {:else if !running && view.context.kind === "section" && info?.section && view.context.blockId !== info.section.blockId}
      <button class="text-button refresh-context" onclick={() => chooseContext("section")}>Use section: {info.section.title}</button>
    {/if}
    {#if selection}<blockquote class="selection-preview">{selection.text}</blockquote>
    {:else if blockPreview}<blockquote class="selection-preview">{blockPreview}</blockquote>{/if}
    <form class="conversation-composer" role="group" aria-label="Chat composer" onsubmit={(event) => { event.preventDefault(); void send(); }}>
      <textarea bind:this={inputEl} aria-label="Message" placeholder="Ask a question…" rows="2" value={view.draft}
        disabled={running} onblur={onInputBlur}
        oninput={(event) => { thread.draft = event.currentTarget.value; updateAssistantConversation(); }}
        onkeydown={(event) => {
          if (event.key === "Enter" && !event.shiftKey && !event.ctrlKey && !event.altKey && !event.metaKey && !event.isComposing) {
            event.preventDefault(); void send();
          }
        }}></textarea>
      <div class="composer-options" bind:this={footerEl}>
        <label>Context
          <select aria-label="Context" value={view.context.kind} disabled={running} onchange={(event) => chooseContext(event.currentTarget.value)}>
            <option value="selection" disabled={!view.selection || !!view.selectionError}>Selection</option>
            <option value="block" disabled={!focusedBlockId}>Block including children</option>
            <option value="section" disabled={!info?.section}>Section / Chapter</option>
            <option value="page" disabled={!contextPageId}>{pageLabel}</option>
            <option value="book" disabled={!info?.book && !info?.isBook}>Whole book</option>
            <option value="graph">My graph</option>
            <option value="none">No notes</option>
          </select>
        </label>
        <label>Mode
          <select aria-label="Mode" value={view.mode} disabled={running} title={assistantModes[view.mode].description}
            onchange={(event) => { thread.mode = event.currentTarget.value as AssistantMode; updateAssistantConversation(); }}>
            {#each Object.entries(assistantModes) as [mode, choice]}<option value={mode}>{choice.label}</option>{/each}
          </select>
        </label>
        {#if running}<button type="button" class="send-button stop-button" onclick={() => void stopAssistantConversation(thread)}>Stop</button>
        {:else}<button class="send-button" type="submit" disabled={!view.draft.trim() || !connected || checking || scopeUnavailable || planning}>{planning ? "Planning…" : "Send"}</button>{/if}
      </div>
    </form>
    <p class="mode-hint">{assistantModes[view.mode].description}</p>
    <details class="privacy-note assistant-disclosure">
      <summary>Model &amp; web privacy</summary>
      <div class="assistant-disclosure-body">
        {#if view.context.kind === "none" && view.messages.length}
          <p>No notes excludes earlier note-backed answers from the next request. They remain visible in this transcript.</p>
        {/if}
        <p>Prompts and selected excerpts go to the configured model. For web modes, Grafium contacts search engines and websites, then forwards results to your model server or service—even if that server has no internet. Endpoint location alone does not guarantee privacy. Grafium never switches models automatically.</p>
      </div>
    </details>
  </div>
</section>

<style>
  .assistant-conversation { display: flex; flex-direction: column; flex: 1; height: 100%; min-width: 0; min-height: 0; gap: 10px; color: var(--text-primary); font-size: 13px; container-type: inline-size; }
  .conversation-header { display: flex; flex-wrap: wrap; align-items: start; justify-content: space-between; gap: 8px; flex-shrink: 0; }
  .source-heading { min-width: 0; flex: 1 1 180px; display: grid; gap: 4px; }
  .source-heading strong { font-size: 15px; overflow-wrap: anywhere; }
  .header-actions { display: flex; flex-wrap: wrap; gap: 6px; }
  button, textarea, select { font: inherit; }
  button { cursor: pointer; border-radius: 5px; }
  button:disabled, select:disabled { cursor: default; opacity: .55; }
  button:focus-visible, select:focus-visible, textarea:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .quiet-button { color: var(--text-primary); background: var(--bg-primary); border: 1px solid var(--border); padding: 5px 8px; }
  .quiet-button:hover:not(:disabled) { background: var(--bg-hover); }
  .model-badge { text-align: left; border: 0; padding: 0; background: transparent; color: var(--text-secondary); font-size: 12px; overflow-wrap: anywhere; }
  .model-badge:hover { color: var(--accent); }
  .model-badge span { display: inline; }
  .conversation-scroll { flex: 1; min-height: 40px; overflow-y: auto; overflow-x: hidden; display: flex; flex-direction: column; gap: 12px; overscroll-behavior: contain; }
  .conversation-scroll > :global(*) { flex-shrink: 0; }
  .conversation-turn { min-width: 0; }
  .conversation-scroll :global(.msg) { min-width: 0; overflow-wrap: anywhere; }
  .conversation-scroll :global(.msg pre), .conversation-scroll :global(.msg table) { max-width: 100%; overflow-x: auto; }
  .answer-badges { display: flex; flex-wrap: wrap; gap: 4px 12px; color: var(--text-secondary); font-size: 11px; margin-bottom: 5px; }
  .empty-message { padding: 24px 2px; line-height: 1.55; color: var(--text-secondary); }
  .empty-message p { color: var(--text-primary); font-weight: 600; margin: 0 0 6px; }
  .shortcuts { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 14px; }
  .conversation-controls { flex-shrink: 0; min-height: 0; display: flex; flex-direction: column; gap: 6px; min-width: 0; max-height: 62%; overflow-y: auto; }
  .context-preview { display: flex; justify-content: space-between; gap: 8px; color: var(--text-secondary); font-size: 12px; }
  .context-preview > span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .text-button { border: none; background: transparent; color: var(--accent); padding: 0; text-decoration: underline; flex-shrink: 0; }
  .refresh-context { align-self: flex-start; font-size: 12px; }
  .selection-preview { margin: 0; font-size: 12px; line-height: 1.4; max-height: 56px; overflow: auto; border-left: 1px solid var(--border); padding-left: 8px; color: var(--text-secondary); overflow-wrap: anywhere; flex-shrink: 0; }
  .conversation-composer { border: 1px solid var(--border); background: var(--bg-primary); border-radius: 7px; min-width: 0; flex-shrink: 0; }
  .conversation-composer:focus-within { border-color: var(--accent); }
  textarea { display: block; box-sizing: border-box; width: 100%; height: 64px; min-height: 32px; resize: none; border: none; padding: 10px; color: var(--text-primary); background: transparent; line-height: 1.5; }
  textarea::placeholder { color: var(--text-secondary); }
  .composer-options { display: flex; flex-wrap: wrap; align-items: end; gap: 8px; padding: 6px 8px 8px; }
  .composer-options label { display: grid; flex: 1 1 150px; min-width: 0; gap: 4px; color: var(--text-secondary); font-size: 12px; }
  select { width: 100%; min-width: 0; color: var(--text-primary); background: var(--bg-secondary); border: 1px solid var(--border); padding: 5px; border-radius: 4px; }
  .send-button { border: 1px solid transparent; padding: 6px 12px; margin-left: auto; color: var(--btn-primary-fg, var(--bg-primary)); background: var(--btn-primary-bg, var(--accent)); }
  .send-button:hover:not(:disabled) { filter: brightness(1.1); }
  .stop-button { background: var(--bg-secondary); color: var(--text-primary); border-color: var(--border); }
  .error-message { color: var(--danger, #c0392b); overflow-wrap: anywhere; margin: 0; font-size: 12px; line-height: 1.5; }
  .connection-notice, .mode-hint, .privacy-note, .status-message { margin: 0; line-height: 1.5; overflow-wrap: anywhere; font-size: 12px; color: var(--text-secondary); }
  .privacy-note { flex-shrink: 0; }
  .privacy-note p { margin: 0; }
  .privacy-note p + p { margin-top: 8px; }
  @container (max-width: 380px) { .composer-options label { flex-basis: calc(50% - 8px); } .model-badge span { display: none; } }
  @media (max-height: 550px) { .selection-preview { max-height: 28px; } .mode-hint { display: none; } .assistant-conversation { gap: 6px; } }
</style>
