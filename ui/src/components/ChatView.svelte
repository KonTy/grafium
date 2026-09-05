<script lang="ts">
  import { onMount } from "svelte";
  import {
    aiAskStream,
    aiHealthCheck,
    aiIndexStatus,
    aiIndexAllPages,
    formatSourceLabel,
    shouldShowIndexBanner,
    type ChatSource,
  } from "../lib/knowledge";

  interface Props {
    onOpenSettings?: () => void;
  }

  let { onOpenSettings = () => {} }: Props = $props();

  type ChatMessage = {
    role: "user" | "assistant";
    content: string;
    sources?: ChatSource[];
  };

  let messages = $state<ChatMessage[]>([
    {
      role: "assistant",
      content:
        "Ask me anything — about your graph or in general. For questions about your notes I'll cite the pages I used; for everything else I'll answer from general knowledge and say so.",
    },
  ]);
  let question = $state("");
  let isStreaming = $state(false);
  let error = $state<string | null>(null);
  let chatScroll: HTMLDivElement | null = null;
  let inputEl: HTMLTextAreaElement | null = null;
  let checkingConnection = $state(true);
  let chatConnected = $state(false);

  // Index coverage — drives the empty-index banner.
  let indexedChunks = $state<number | null>(null);
  let totalBlocks = $state(0);
  let embedderReady = $state(false);
  let isIndexing = $state(false);
  let indexError = $state<string | null>(null);
  let indexJustFinished = $state(false);
  let indexResult = $state<{ processed: number; failed: number } | null>(null);
  let statusError = $state<string | null>(null);

  let indexEmpty = $derived(shouldShowIndexBanner(indexedChunks));

  onMount(() => {
    keepInputFocusedSoon(true);
    void refreshConnectionState();
    void refreshIndexStatus();
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
      embedderReady = status.embedder_ready;
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
    isStreaming = true;
    await scrollToBottom();

    let assistantIndex = messages.length - 1;

    await aiAskStream(
      trimmed,
      {
        onChunk: (delta) => {
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
        onDone: () => {
          isStreaming = false;
          keepInputFocusedSoon(false);
          void scrollToBottom();
        },
        onError: (msg) => {
          error = msg;
          isStreaming = false;
          keepInputFocusedSoon(false);
        },
      }
    );
  }

  function onInputKeydown(e: KeyboardEvent) {
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
  {/if}

  <div class="chat-log" bind:this={chatScroll}>
    {#each messages as m}
      <div class="msg" class:user={m.role === "user"}>
        <div class="msg-role">{m.role === "user" ? "You" : "Grafium AI"}</div>
        <div class="msg-content">{m.content || (isStreaming ? "..." : "")}</div>
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
      </div>
    {/each}
  </div>

  {#if error}
    <div class="chat-error">{error}</div>
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

  .chat-error {
    color: #f87171;
    font-size: 12px;
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
