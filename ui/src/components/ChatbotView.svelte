<script lang="ts">
  import { onMount } from "svelte";
  import { aiAskStream, aiHealthCheck } from "../lib/knowledge";

  interface Props {
    onOpenSettings?: () => void;
  }

  let { onOpenSettings = () => {} }: Props = $props();

  type ChatMessage = {
    role: "user" | "assistant";
    content: string;
  };

  let messages = $state<ChatMessage[]>([
    {
      role: "assistant",
      content:
        "Ask me anything about your graph. I can analyze topics and suggest hidden connections based on semantic similarity.",
    },
  ]);
  let question = $state("");
  let isStreaming = $state(false);
  let error = $state<string | null>(null);
  let chatScroll: HTMLDivElement | null = null;
  let inputEl: HTMLTextAreaElement | null = null;
  let checkingConnection = $state(true);
  let chatbotConnected = $state(false);

  onMount(() => {
    keepInputFocusedSoon(true);
    void refreshConnectionState();
  });

  $effect(() => {
    if (!checkingConnection && chatbotConnected && !isStreaming) {
      keepInputFocusedSoon(true);
    }
  });

  async function refreshConnectionState() {
    checkingConnection = true;
    try {
      const status = await aiHealthCheck();
      chatbotConnected = status.enabled && status.llm_available;
    } catch {
      chatbotConnected = false;
    } finally {
      checkingConnection = false;
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
    if (isStreaming || (!checkingConnection && !chatbotConnected)) return;
    keepInputFocusedSoon(false);
  }

  async function send() {
    const trimmed = question.trim();
    if (!trimmed || isStreaming) return;

    if (checkingConnection) {
      error = "Checking chatbot connection. Please wait a moment.";
      return;
    }

    if (!chatbotConnected) {
      error = "Before you talk to chatbot, first you need to connect to it in Settings.";
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
    <h2>Chatbot</h2>
    <p>Streaming local/cloud assistant for graph analysis</p>
  </header>

  {#if checkingConnection}
    <div class="chat-status">Checking chatbot connection...</div>
  {:else if !chatbotConnected}
    <div class="chat-warning">
      <p>Before you talk to chatbot, first you need to connect to it.</p>
      <button class="settings-link" onclick={() => onOpenSettings()}>Open Settings</button>
    </div>
  {/if}

  <div class="chat-log" bind:this={chatScroll}>
    {#each messages as m}
      <div class="msg" class:user={m.role === "user"}>
        <div class="msg-role">{m.role === "user" ? "You" : "Grafium AI"}</div>
        <div class="msg-content">{m.content || (isStreaming ? "..." : "")}</div>
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
      disabled={isStreaming || (!checkingConnection && !chatbotConnected)}
    ></textarea>
    <button onclick={() => void send()} disabled={isStreaming || !question.trim() || checkingConnection || !chatbotConnected}>
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
