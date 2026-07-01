<script lang="ts">
  import { aiAskStream } from "../lib/knowledge";

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

  async function send() {
    const trimmed = question.trim();
    if (!trimmed || isStreaming) return;

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
          void scrollToBottom();
        },
        onError: (msg) => {
          error = msg;
          isStreaming = false;
        },
      }
    );
  }

  function onInputKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
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
      placeholder="Ask about relationships, themes, or missing links in your graph..."
      rows="3"
      onkeydown={onInputKeydown}
      disabled={isStreaming}
    ></textarea>
    <button onclick={() => void send()} disabled={isStreaming || !question.trim()}>
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
