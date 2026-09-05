<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { open as openExternal } from "@tauri-apps/plugin-shell";
  import { renderAssistantMarkdown } from "../lib/markdown";
  import {
    aiAskStream,
    aiCancelStream,
    aiHealthCheck,
    aiIndexStatus,
    aiIndexAllPages,
    aiRetryLlmOnGpu,
    formatSourceLabel,
    formatWebSourceLabel,
    shouldShowIndexBanner,
    type AcceleratorStatus,
    type ChatSource,
    type WebSource,
  } from "../lib/knowledge";
  import {
    initialState,
    reduce,
    statusDisplay,
    type StreamState,
    type StreamEvent,
    type StreamPhase,
  } from "../lib/chatStatus";

  interface Props {
    onOpenSettings?: () => void;
  }

  let { onOpenSettings = () => {} }: Props = $props();

  type ChatMessage = {
    role: "user" | "assistant";
    content: string;
    sources?: ChatSource[];
    /** Web citations for a research answer's "From the web" section. */
    webSources?: WebSource[];
    /** True once this answer engaged web research — drives the "Web research" badge. */
    webResearch?: boolean;
  };

  let messages = $state<ChatMessage[]>([
    {
      role: "assistant",
      content:
        "Ask me anything — about your graph or in general. For questions about your notes I'll cite the pages I used; for everything else I'll answer from general knowledge and say so.",
    },
  ]);
  let question = $state("");
  let currentRequestId: string | null = null;
  let error = $state<string | null>(null);

  // Streaming status is a pure reducer over the *real* backend events
  // (phase transitions, token deltas, done/error) plus a wall clock — never a
  // timer-driven spinner. `now` is bumped by a lightweight clock only so the
  // elapsed time and stall detection recompute; every animated/"working" state
  // is still gated on actual evidence in `statusDisplay`.
  let streamState = $state<StreamState>(initialState());
  let now = $state(0);
  let clock: ReturnType<typeof setInterval> | null = null;
  let reducedMotion = $state(false);

  // Transient, human-readable detail for the current web-research step (e.g.
  // "Reading source 2/5: example.com"). Display-only — shown under the status
  // label while streaming and cleared when the answer finishes. Kept out of the
  // status reducer so the well-tested liveness machine stays lean.
  let webNote = $state("");

  let status = $derived(statusDisplay(streamState, now, reducedMotion));
  let isStreaming = $derived(
    streamState.kind === "active" || streamState.kind === "stalled"
  );

  let chatScroll: HTMLDivElement | null = null;
  let inputEl: HTMLTextAreaElement | null = null;
  let checkingConnection = $state(true);
  let chatConnected = $state(false);

  // Index coverage — drives the empty-index banner.
  let indexedChunks = $state<number | null>(null);
  let totalBlocks = $state(0);
  let pendingPages = $state(0);
  let embedderReady = $state(false);
  let isIndexing = $state(false);
  let indexError = $state<string | null>(null);
  let indexJustFinished = $state(false);
  let indexResult = $state<{ processed: number; failed: number } | null>(null);
  let statusError = $state<string | null>(null);

  // Local-LLM GPU/CPU status — drives the "Running on CPU" warning banner.
  let accelerator = $state<AcceleratorStatus | null>(null);
  let retryingGpu = $state(false);
  let retryGpuError = $state<string | null>(null);

  // Only warn when the GPU is genuinely available but unused; a build with no
  // GPU backend runs on CPU by design and must not nag.
  let cpuFallback = $derived(
    accelerator !== null && accelerator.gpu_supported && !accelerator.on_gpu
  );

  let indexEmpty = $derived(shouldShowIndexBanner(indexedChunks));

  onMount(() => {
    keepInputFocusedSoon(true);
    void refreshConnectionState();
    void refreshIndexStatus();

    // Honour the OS "reduce motion" preference: fall back to a static label
    // instead of animating. Kept reactive so a mid-session change applies.
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    reducedMotion = mq.matches;
    const onMotionChange = (e: MediaQueryListEvent) => {
      reducedMotion = e.matches;
    };
    mq.addEventListener("change", onMotionChange);

    // The background auto-reindex drainer emits this after it refreshes any
    // pages, so the coverage / "N pages pending" indicator stays current
    // without polling.
    const unlistenPromise = listen("ai-index-updated", () => {
      void refreshIndexStatus();
    });

    // Delegated click handling for links inside rendered assistant markdown.
    // Attached programmatically (rather than an inline handler on the div) so
    // a container-level listener doesn't trip the a11y lints meant for
    // interactive elements.
    chatScroll?.addEventListener("click", handleRenderedClick);

    return () => {
      void unlistenPromise.then((unlisten) => unlisten());
      mq.removeEventListener("change", onMotionChange);
      chatScroll?.removeEventListener("click", handleRenderedClick);
      stopClock();
    };
  });

  // Run the clock only while a stream is in flight, so `now` (and thus elapsed
  // time + stall detection) stays live without polling when idle.
  $effect(() => {
    if (isStreaming) {
      startClock();
    } else {
      stopClock();
    }
  });

  $effect(() => {
    if (!checkingConnection && chatConnected && !isStreaming) {
      keepInputFocusedSoon(true);
    }
  });

  async function refreshConnectionState() {
    checkingConnection = true;
    try {
      const status = await aiHealthCheck();
      chatConnected = status.enabled && status.llm_available;
    } catch {
      chatConnected = false;
    } finally {
      checkingConnection = false;
    }
  }

  async function refreshIndexStatus() {
    try {
      const status = await aiIndexStatus();
      indexedChunks = status.indexed_chunks;
      totalBlocks = status.total_blocks;
      pendingPages = status.pending_pages;
      embedderReady = status.embedder_ready;
      accelerator = status.accelerator;
      statusError = null;
    } catch (e: any) {
      // Surface the failure instead of silently hiding the banner — a status
      // error is itself actionable (e.g. no embedder configured).
      statusError = String(e);
      indexedChunks = null;
    }
  }

  async function startIndexing() {
    if (isIndexing) return;
    indexError = null;
    indexJustFinished = false;
    indexResult = null;
    isIndexing = true;
    try {
      const result = await aiIndexAllPages();
      indexResult = {
        processed: result.pages_processed,
        failed: result.pages_failed,
      };
      await refreshIndexStatus();
      indexJustFinished = true;
    } catch (e: any) {
      indexError = String(e);
    } finally {
      isIndexing = false;
    }
  }

  async function retryOnGpu() {
    if (retryingGpu) return;
    retryGpuError = null;
    retryingGpu = true;
    try {
      accelerator = await aiRetryLlmOnGpu();
    } catch (e: any) {
      retryGpuError = String(e);
    } finally {
      retryingGpu = false;
    }
  }

  function openSource(source: ChatSource) {
    window.dispatchEvent(
      new CustomEvent("navigate-page", {
        detail: {
          pageName: source.page_title,
          sourceBlockId: source.block_id,
          sourcePageTitle: source.page_title,
          targetBlockId: source.block_id,
        },
      })
    );
  }

  // Open a web citation in the system browser via the shell plugin — the same
  // mechanism as external links inside rendered answers. Never navigate the
  // webview itself away from the app.
  function openWebSource(source: WebSource) {
    if (/^https?:\/\//i.test(source.url)) {
      openExternal(source.url).catch(() => {});
    }
  }

  // Delegated handler for links inside rendered assistant markdown. Mirrors
  // PageContent/BlockEditor: `[[page]]`/`#tag` anchors (emitted by
  // `renderAssistantMarkdown` as `<a class="page-link" data-page>` /
  // `<a class="tag" data-tag>`) dispatch the existing `navigate-page` event;
  // external `http(s)` links open in the system browser via the shell plugin
  // instead of navigating the webview away from the app. Everything else is
  // swallowed (preventDefault) so an unexpected/blocked scheme can't navigate.
  function handleRenderedClick(e: MouseEvent) {
    const anchor = (e.target as HTMLElement).closest("a");
    if (!anchor) return;

    if (anchor.classList.contains("page-link")) {
      e.preventDefault();
      const pageName = anchor.dataset.page;
      if (pageName) {
        window.dispatchEvent(
          new CustomEvent("navigate-page", { detail: { pageName } })
        );
      }
      return;
    }

    if (anchor.classList.contains("tag")) {
      e.preventDefault();
      const tag = anchor.dataset.tag;
      if (tag) {
        window.dispatchEvent(
          new CustomEvent("navigate-page", { detail: { pageName: tag } })
        );
      }
      return;
    }

    // Any other anchor is an ordinary markdown link. Never let it navigate
    // the webview; open real web links externally.
    e.preventDefault();
    const href = anchor.getAttribute("href") ?? "";
    if (/^https?:\/\//i.test(href)) {
      openExternal(href).catch(() => {});
    }
  }

  function focusInput(select = false) {
    if (!inputEl || inputEl.disabled) return;
    inputEl.focus();
    if (select) inputEl.select();
  }

  function keepInputFocusedSoon(select = false) {
    requestAnimationFrame(() => focusInput(select));
  }

  function onInputBlur() {
    if (isStreaming || (!checkingConnection && !chatConnected)) return;
    keepInputFocusedSoon(false);
  }

  async function send() {
    const trimmed = question.trim();
    if (!trimmed || isStreaming) return;

    if (checkingConnection) {
      error = "Checking Chat connection. Please wait a moment.";
      return;
    }

    if (!chatConnected) {
      error = "Before you use Chat, first you need to connect to it in Settings.";
      return;
    }

    error = null;
    messages = [...messages, { role: "user", content: trimmed }, { role: "assistant", content: "" }];
    question = "";
    webNote = "";
    // Begin the status machine at the real send time.
    dispatch({ type: "start", at: Date.now() });
    await scrollToBottom();

    let assistantIndex = messages.length - 1;

    await aiAskStream(
      trimmed,
      {
        onStart: (requestId) => {
          currentRequestId = requestId;
        },
        onPhase: (phase) => {
          dispatch({ type: "phase", phase: phase as StreamPhase, at: Date.now() });
          // Entering a web phase means this answer engaged research — light up
          // the badge even before the citations land.
          if (phase === "searching_web" || phase === "reading_sources") {
            messages = messages.map((m, i) =>
              i === assistantIndex ? { ...m, webResearch: true } : m
            );
          }
        },
        onNote: (note) => {
          // Real progress evidence (keeps the liveness clock alive) plus the
          // detail line under the status label.
          dispatch({ type: "note", at: Date.now() });
          webNote = note;
        },
        onChunk: (delta) => {
          dispatch({ type: "delta", chars: delta.length, at: Date.now() });
          messages = messages.map((m, i) => {
            if (i !== assistantIndex) return m;
            return { ...m, content: m.content + delta };
          });
          void scrollToBottom();
        },
        onSources: (sources) => {
          messages = messages.map((m, i) => {
            if (i !== assistantIndex) return m;
            return { ...m, sources };
          });
        },
        onWebSources: (webSources) => {
          messages = messages.map((m, i) => {
            if (i !== assistantIndex) return m;
            return { ...m, webSources, webResearch: true };
          });
        },
        onDone: () => {
          dispatch({ type: "done", at: Date.now() });
          webNote = "";
          // If the model finished without emitting anything, say so plainly
          // rather than leaving an empty bubble.
          if (streamState.firstTokenAt === null) {
            messages = messages.map((m, i) => {
              if (i !== assistantIndex || m.content) return m;
              return {
                ...m,
                content:
                  "The model returned no answer. Try rephrasing, or use a smaller/non-reasoning model.",
              };
            });
          }
          keepInputFocusedSoon(false);
          void scrollToBottom();
        },
        onError: (msg) => {
          error = msg;
          webNote = "";
          dispatch({ type: "error", at: Date.now(), message: msg });
          keepInputFocusedSoon(false);
        },
      }
    );
  }

  function dispatch(e: StreamEvent) {
    now = e.at;
    streamState = reduce(streamState, e);
  }

  function startClock() {
    if (clock) return;
    now = Date.now();
    clock = setInterval(() => {
      now = Date.now();
    }, 250);
  }

  function stopClock() {
    if (clock) {
      clearInterval(clock);
      clock = null;
    }
  }

  function stopStream() {
    if (currentRequestId) {
      void aiCancelStream(currentRequestId);
    }
    // Reflect the user's intent immediately; any partial answer already
    // streamed stays in the bubble. A trailing backend `done` is ignored once
    // we're in a terminal state.
    dispatch({ type: "cancel", at: Date.now() });
    keepInputFocusedSoon(false);
  }  function onInputKeydown(e: KeyboardEvent) {
    const hasModifier = e.shiftKey || e.ctrlKey || e.altKey || e.metaKey;
    if (e.key === "Enter" && !hasModifier) {
      e.preventDefault();
      void send();
    }
  }

  async function scrollToBottom() {
    await Promise.resolve();
    if (chatScroll) {
      chatScroll.scrollTop = chatScroll.scrollHeight;
    }
  }
</script>

<div class="chat-view">
  <header class="chat-header">
    <h2>Chat</h2>
    <p>Streaming local/cloud assistant for graph analysis</p>
  </header>

  {#if checkingConnection}
    <div class="chat-status">Checking Chat connection...</div>
  {:else if !chatConnected}
    <div class="chat-warning">
      <p>Before you use Chat, first you need to connect to it.</p>
      <button class="settings-link" onclick={() => onOpenSettings()}>Open Settings</button>
    </div>
  {/if}

  {#if statusError}
    <div class="index-banner">
      <div class="index-banner-text">
        <strong>Couldn't check your index status</strong>
        <span class="index-error">{statusError}</span>
      </div>
      <button class="index-button" onclick={() => onOpenSettings()}>
        Open Settings
      </button>
    </div>
  {:else if indexEmpty && !indexJustFinished}
    <div class="index-banner">
      <div class="index-banner-text">
        <strong>Your notes aren't indexed yet</strong>
        <span>
          Chat can only use general knowledge and text search until you build the
          semantic index{totalBlocks > 0 ? ` (${totalBlocks} blocks)` : ""}.
        </span>
        {#if indexError}<span class="index-error">{indexError}</span>{/if}
      </div>
      {#if embedderReady}
        <button
          class="index-button"
          onclick={() => void startIndexing()}
          disabled={isIndexing}
        >
          {isIndexing ? "Indexing…" : "Index my notes"}
        </button>
      {:else}
        <button class="index-button" onclick={() => onOpenSettings()}>
          Configure embedding model
        </button>
      {/if}
    </div>
  {:else if indexJustFinished}
    <div class="index-done">
      Indexed {indexedChunks} chunk{indexedChunks === 1 ? "" : "s"}{indexResult
        ? ` from ${indexResult.processed} page${indexResult.processed === 1 ? "" : "s"}${indexResult.failed > 0 ? `, ${indexResult.failed} failed` : ""}`
        : ""} — semantic search is on.
    </div>
  {:else if pendingPages > 0}
    <div class="index-pending" title="Recently edited pages are being re-indexed in the background.">
      {pendingPages} page{pendingPages === 1 ? "" : "s"} updating in the background…
    </div>
  {/if}

  {#if cpuFallback}
    <div class="cpu-banner" role="status">
      <div class="index-banner-text">
        <strong>Running on CPU — responses will be slow</strong>
        <span>
          The GPU had only {accelerator?.free_vram_mib_at_load ?? "?"} MiB free
          when the model loaded{accelerator?.model_mib
            ? ` (it needs roughly ${accelerator.model_mib} MiB)`
            : ""}. If something else was using the GPU at startup, free it and
          retry.
        </span>
        {#if retryGpuError}<span class="index-error">{retryGpuError}</span>{/if}
      </div>
      <button
        class="index-button"
        onclick={() => void retryOnGpu()}
        disabled={retryingGpu}
      >
        {retryingGpu ? "Retrying…" : "Retry on GPU"}
      </button>
    </div>
  {/if}

  <div class="chat-log" bind:this={chatScroll}>
    {#each messages as m, i}
      {@const streamingThis =
        isStreaming && m.role === "assistant" && i === messages.length - 1}
      <div class="msg" class:user={m.role === "user"}>
        <div class="msg-role">
          {m.role === "user" ? "You" : "Grafium AI"}
          {#if m.role === "assistant" && m.webResearch}
            <span class="research-badge" title="This answer includes live web research">
              <span class="research-badge-dot" aria-hidden="true"></span>Web research
            </span>
          {/if}
        </div>
        {#if m.role === "assistant" && !streamingThis}
          <!-- Completed assistant answers render as markdown (bold, lists,
               code, KaTeX, clickable [[links]]/#tags). User input and the
               in-flight streaming bubble stay plain text — rendering partial
               markdown per token would reparse on every delta and could show
               broken half-syntax. -->
          <div class="msg-content markdown">{@html renderAssistantMarkdown(m.content)}</div>
        {:else}
          <div class="msg-content">{m.content}{#if streamingThis}<span
                class="type-cursor"
                class:animate={status.animate}
                aria-hidden="true"
              ></span>{/if}</div>
        {/if}
        {#if m.role === "assistant" && m.sources && m.sources.length > 0}
          <div class="msg-sources">
            {#each m.sources as source}
              <button
                class="source-chip"
                onclick={() => openSource(source)}
                title={`Open ${formatSourceLabel(source)}`}
              >
                <span class="source-index">[{source.index}]</span>
                <span class="source-title">{source.page_title}</span>
                {#if source.date}<span class="source-date">{source.date}</span>{/if}
              </button>
            {/each}
          </div>
        {/if}
        {#if m.role === "assistant" && m.webSources && m.webSources.length > 0}
          <!-- Web citations for the "From the web" section. Rendered distinctly
               from graph chips (external-link styling + ↗) and opened in the
               system browser, never in the webview. -->
          <div class="msg-sources web">
            {#each m.webSources as source}
              <button
                class="source-chip web-source-chip"
                onclick={() => openWebSource(source)}
                title={`Open ${source.url}`}
              >
                <span class="source-index">[{source.number}]</span>
                <span class="source-title">{formatWebSourceLabel(source).replace(/^\[\d+\]\s*/, "")}</span>
                <span class="source-ext" aria-hidden="true">↗</span>
              </button>
            {/each}
          </div>
        {/if}
      </div>
    {/each}
  </div>

  {#if error}
    <div class="chat-error">{error}</div>
  {/if}

  {#if isStreaming}
    <div class="chat-status" role="status" aria-live="polite" class:stalled={status.kind === "stalled"}>
      <span
        class="chat-status-dot"
        class:animate={status.animate}
        class:thinking={status.phase === "thinking"}
        class:stalled={status.kind === "stalled"}
      ></span>
      <span class="chat-status-label">{status.label}</span>
      {#if webNote && status.kind !== "stalled"}
        <span class="chat-status-note" title={webNote}>{webNote}</span>
      {/if}
      {#if status.showStop}
        <button class="chat-stop" onclick={() => stopStream()} title="Stop generating">
          Stop
        </button>
      {/if}
    </div>
  {/if}

  <div class="chat-input-row">
    <textarea
      bind:value={question}
      bind:this={inputEl}
      placeholder="Ask about relationships, themes, or missing links in your graph..."
      rows="3"
      onkeydown={onInputKeydown}
      onblur={onInputBlur}
      disabled={isStreaming || (!checkingConnection && !chatConnected)}
    ></textarea>
    <button onclick={() => void send()} disabled={isStreaming || !question.trim() || checkingConnection || !chatConnected}>
      {isStreaming ? "Streaming..." : "Send"}
    </button>
  </div>
</div>

<style>
  .chat-view {
    display: flex;
    flex-direction: column;
    height: 100%;
    padding: 18px;
    gap: 12px;
    background: var(--bg-primary);
  }

  .chat-header h2 {
    margin: 0;
    font-size: 22px;
  }

  .chat-header p {
    margin: 4px 0 0;
    color: var(--text-muted);
    font-size: 12px;
  }

  .chat-log {
    flex: 1;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg-secondary);
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .chat-status,
  .chat-warning {
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg-secondary);
    padding: 10px 12px;
  }

  .chat-status {
    color: var(--text-muted);
    font-size: 12px;
  }

  .chat-warning p {
    margin: 0 0 8px;
    color: var(--text-secondary);
    font-size: 13px;
  }

  .settings-link {
    padding: 7px 10px;
    border: 1px solid var(--accent);
    border-radius: 6px;
    background: transparent;
    color: var(--accent);
    cursor: pointer;
    font-size: 12px;
    font-weight: 600;
  }

  .settings-link:hover {
    background: color-mix(in srgb, var(--accent) 14%, transparent);
  }

  .msg {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 10px;
    background: var(--bg-primary);
  }

  .msg.user {
    border-color: var(--accent);
  }

  .msg-role {
    font-size: 11px;
    color: var(--text-muted);
    margin-bottom: 4px;
  }

  .msg-content {
    white-space: pre-wrap;
    word-break: break-word;
    line-height: 1.45;
  }

  /* Rendered assistant markdown: block layout instead of pre-wrap, plus the
     same link/code/list styling page content uses (scoped via :global since
     the HTML is injected with {@html}). */
  .msg-content.markdown {
    white-space: normal;
  }

  .msg-content.markdown :global(p) {
    margin: 0 0 8px;
  }

  .msg-content.markdown :global(p:last-child) {
    margin-bottom: 0;
  }

  .msg-content.markdown :global(ul),
  .msg-content.markdown :global(ol) {
    margin: 4px 0 8px;
    padding-left: 22px;
  }

  .msg-content.markdown :global(li) {
    margin: 2px 0;
  }

  .msg-content.markdown :global(h1),
  .msg-content.markdown :global(h2),
  .msg-content.markdown :global(h3),
  .msg-content.markdown :global(h4) {
    margin: 12px 0 6px;
    line-height: 1.3;
  }

  .msg-content.markdown :global(blockquote) {
    margin: 6px 0;
    padding-left: 12px;
    border-left: 3px solid var(--border);
    color: var(--text-secondary);
  }

  .msg-content.markdown :global(code) {
    background: var(--bg-code);
    padding: 1px 4px;
    border-radius: 4px;
    font-family: 'JetBrains Mono', 'Fira Code', monospace;
    font-size: 0.9em;
  }

  .msg-content.markdown :global(.code-block-wrapper) {
    position: relative;
    background: var(--bg-code);
    border-radius: 6px;
    margin: 6px 0;
    overflow: hidden;
  }

  .msg-content.markdown :global(.code-lang) {
    position: absolute;
    top: 4px;
    right: 8px;
    font-size: 11px;
    color: var(--text-muted);
  }

  .msg-content.markdown :global(.code-block-pre) {
    margin: 0;
    padding: 10px 12px;
    background: none;
    overflow-x: auto;
    counter-reset: codeline;
  }

  .msg-content.markdown :global(.code-block-pre code) {
    background: none;
    padding: 0;
    font-size: 13px;
    line-height: 1.5;
  }

  .msg-content.markdown :global(.code-line) {
    display: block;
    counter-increment: codeline;
  }

  .msg-content.markdown :global(.code-line)::before {
    content: counter(codeline);
    display: inline-block;
    width: 2em;
    margin-right: 1em;
    text-align: right;
    color: var(--text-muted);
    user-select: none;
  }

  .msg-content.markdown :global(.page-link),
  .msg-content.markdown :global(a) {
    color: var(--text-link);
    cursor: pointer;
    text-decoration: none;
    border-bottom: 1px solid transparent;
  }

  .msg-content.markdown :global(.page-link:hover),
  .msg-content.markdown :global(a:hover) {
    color: var(--text-link-hover);
    border-bottom-color: var(--text-link-hover);
  }

  .msg-content.markdown :global(.tag) {
    color: var(--accent-secondary);
    cursor: pointer;
    text-decoration: none;
  }

  .msg-content.markdown :global(img) {
    max-width: 100%;
    height: auto;
    border-radius: 6px;
  }

  .msg-sources {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 8px;
  }

  .source-chip {
    display: inline-flex;
    align-items: baseline;
    gap: 6px;
    padding: 3px 8px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--bg-secondary);
    color: var(--text-secondary);
    cursor: pointer;
    font-size: 11px;
    max-width: 100%;
  }

  .source-chip:hover {
    border-color: var(--accent);
    color: var(--text-primary);
  }

  .source-index {
    color: var(--text-muted);
    font-weight: 600;
  }

  .source-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 220px;
  }

  .source-date {
    color: var(--text-muted);
  }

  /* Web-research affordances share the external-link visual language: the
     --accent-cyan token and an outbound ↗ arrow, so a web citation reads as
     "leaves the app" and is clearly distinct from a graph page chip. */
  .research-badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-left: 8px;
    padding: 1px 7px;
    border: 1px solid color-mix(in srgb, var(--accent-cyan) 45%, transparent);
    border-radius: 999px;
    background: color-mix(in srgb, var(--accent-cyan) 12%, transparent);
    color: var(--accent-cyan);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.02em;
    vertical-align: middle;
  }

  .research-badge-dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--accent-cyan);
  }

  .web-source-chip {
    border-color: color-mix(in srgb, var(--accent-cyan) 40%, var(--border));
    color: var(--accent-cyan);
  }

  .web-source-chip:hover {
    border-color: var(--accent-cyan);
    color: var(--accent-cyan);
    background: color-mix(in srgb, var(--accent-cyan) 10%, var(--bg-secondary));
  }

  .web-source-chip .source-index {
    color: color-mix(in srgb, var(--accent-cyan) 70%, var(--text-muted));
  }

  .source-ext {
    color: var(--accent-cyan);
    font-size: 10px;
  }

  .chat-status-note {
    color: var(--text-muted);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 260px;
  }

  .index-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    border: 1px solid var(--accent);
    border-radius: 10px;
    background: color-mix(in srgb, var(--accent) 8%, var(--bg-secondary));
    padding: 10px 12px;
  }

  .index-banner-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .index-banner-text strong {
    color: var(--text-primary);
    font-size: 13px;
  }

  .index-error {
    color: #f87171;
  }

  /* CPU-fallback warning — same layout as the index banner, but an amber
     accent so it reads as a warning rather than an action prompt. */
  .cpu-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-top: 8px;
    border: 1px solid #d9a441;
    border-radius: 10px;
    background: color-mix(in srgb, #d9a441 10%, var(--bg-secondary));
    padding: 10px 12px;
  }

  .index-button {
    flex-shrink: 0;
    padding: 8px 12px;
    border: none;
    border-radius: 6px;
    background: var(--btn-primary-bg);
    color: var(--btn-primary-fg);
    cursor: pointer;
    font-size: 12px;
    font-weight: 600;
  }

  .index-button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .index-done {
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg-secondary);
    padding: 8px 12px;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .index-pending {
    padding: 2px 4px;
    font-size: 11px;
    color: var(--text-secondary);
    opacity: 0.7;
  }

  .chat-error {
    color: #f87171;
    font-size: 12px;
  }

  .chat-status {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 2px;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .chat-status.stalled {
    color: #fbbf24;
  }

  .chat-status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--text-secondary);
    opacity: 0.6;
    flex-shrink: 0;
  }

  /* Animation runs ONLY when the status is evidence-backed and motion is
     allowed — never on a bare timer. A stalled/terminal dot is static. */
  .chat-status-dot.animate {
    animation: chat-pulse 1.2s ease-in-out infinite;
  }

  .chat-status-dot.thinking {
    background: #a78bfa;
    opacity: 0.9;
  }

  .chat-status-dot.stalled {
    background: #fbbf24;
    opacity: 0.9;
    animation: none;
  }

  @keyframes chat-pulse {
    0%,
    100% {
      opacity: 0.3;
    }
    50% {
      opacity: 0.9;
    }
  }

  /* A subtle "typing" cursor at the end of the streaming answer, so tokens
     appearing feel live. Static (just visible) unless animation is warranted. */
  .type-cursor {
    display: inline-block;
    width: 2px;
    height: 1em;
    margin-left: 1px;
    vertical-align: text-bottom;
    background: var(--text-secondary);
    opacity: 0.5;
  }

  .type-cursor.animate {
    animation: chat-cursor-blink 1s steps(2, start) infinite;
  }

  @keyframes chat-cursor-blink {
    0%,
    100% {
      opacity: 0.15;
    }
    50% {
      opacity: 0.85;
    }
  }

  /* Backstop: honour reduced-motion even if a class slips through. */
  @media (prefers-reduced-motion: reduce) {
    .chat-status-dot.animate,
    .type-cursor.animate {
      animation: none;
    }
  }

  .chat-status-label {
    flex: 1;
  }

  .chat-stop {
    padding: 3px 12px;
    font-size: 12px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-input);
    color: var(--text-primary);
    cursor: pointer;
  }

  .chat-stop:hover {
    border-color: #f87171;
    color: #f87171;
  }

  .chat-input-row {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 10px;
  }

  textarea {
    width: 100%;
    resize: vertical;
    min-height: 72px;
    max-height: 220px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    color: var(--text-primary);
    border-radius: 8px;
    padding: 10px;
    font: inherit;
  }

  button {
    align-self: end;
    padding: 10px 14px;
    border: none;
    border-radius: 8px;
    background: var(--btn-primary-bg);
    color: var(--btn-primary-fg);
    cursor: pointer;
  }

  button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
