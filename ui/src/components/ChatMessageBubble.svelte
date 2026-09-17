<script lang="ts">
  import { open as openExternal } from "@tauri-apps/plugin-shell";
  import { renderAssistantMarkdown } from "../lib/markdown";
  import {
    formatSourceLabel,
    formatWebSourceLabel,
    type ChatSource,
    type WebSource,
  } from "../lib/knowledge";
  import type { ChatMessageModel, ChatThinkingTone } from "../lib/chatMessage";

  interface Props {
    message: ChatMessageModel;
    index?: number;
    streaming?: boolean;
    animateCursor?: boolean;
    thinkingLabel?: string;
    thinkingTone?: ChatThinkingTone;
    /** Set when a step trail above this bubble already reports the phase, so
     *  the placeholder doesn't say "Thinking…" directly under "Thinking". */
    trailed?: boolean;
    onOpenSource?: (source: ChatSource) => void;
    onOpenWebSource?: (source: WebSource) => void;
  }

  let {
    message,
    index = 0,
    streaming = false,
    animateCursor = false,
    thinkingLabel = "Thinking…",
    thinkingTone = "thinking",
    trailed = false,
    onOpenSource = () => {},
    onOpenWebSource = (source: WebSource) => {
      if (/^https?:\/\//i.test(source.url)) {
        openExternal(source.url).catch(() => {});
      }
    },
  }: Props = $props();

  let copied = $state(false);
  let copiedTimer: ReturnType<typeof setTimeout> | null = null;

  let showThinking = $derived(
    message.role === "assistant" && streaming && !message.content.trim() && !trailed,
  );
  // With a step trail above it, an empty bubble adds a second "Grafium AI"
  // heading and a placeholder that says the same thing the trail already says.
  // It appears the moment real text arrives.
  let hideBubble = $derived(
    message.role === "assistant" && streaming && !message.content.trim() && trailed,
  );
  let isCompletedAssistant = $derived(
    message.role === "assistant" && !streaming && message.content.trim().length > 0,
  );

  async function copyMessage() {
    let out = message.content.trim();

    const graphRefs = (message.sources ?? []).map((s) => `- ${formatSourceLabel(s)}`);
    const webRefs = (message.webSources ?? []).map(
      (s) => `- ${formatWebSourceLabel(s)} (${s.url})`,
    );
    if (graphRefs.length || webRefs.length) {
      out += "\n\n**Sources**\n" + [...graphRefs, ...webRefs].join("\n");
    }

    try {
      await navigator.clipboard.writeText(out);
    } catch {
      const scratch = document.createElement("textarea");
      scratch.value = out;
      scratch.style.position = "fixed";
      scratch.style.opacity = "0";
      document.body.appendChild(scratch);
      scratch.select();
      document.execCommand("copy");
      scratch.remove();
    }

    copied = true;
    if (copiedTimer) clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => (copied = false), 1500);
  }

  // Delegated handler for links inside rendered assistant markdown. This keeps
  // Chat and Ask on the same navigation rules for [[page]], #tag, and web links.
  function handleRenderedClick(e: MouseEvent) {
    const target = e.target instanceof Element ? e.target : null;
    const anchor = target?.closest("a");
    if (!anchor) return;

    if (anchor.classList.contains("page-link")) {
      e.preventDefault();
      const pageName = (anchor as HTMLElement).dataset.page;
      if (pageName) {
        window.dispatchEvent(
          new CustomEvent("navigate-page", { detail: { pageName } }),
        );
      }
      return;
    }

    if (anchor.classList.contains("tag")) {
      e.preventDefault();
      const tag = (anchor as HTMLElement).dataset.tag;
      if (tag) {
        window.dispatchEvent(
          new CustomEvent("navigate-page", { detail: { pageName: tag } }),
        );
      }
      return;
    }

    e.preventDefault();
    const href = anchor.getAttribute("href") ?? "";
    if (/^https?:\/\//i.test(href)) {
      openExternal(href).catch(() => {});
    }
  }
</script>

{#if !hideBubble}
<div
  class="msg"
  class:user={message.role === "user"}
  class:thinking={showThinking}
  class:tone-web={showThinking && thinkingTone === "web"}
  class:tone-stalled={showThinking && thinkingTone === "stalled"}
  class:tone-working={showThinking && thinkingTone === "working"}
  data-message-index={index}
>
  <div class="msg-role">
    {message.role === "user" ? "You" : "Grafium AI"}
    {#if message.role === "assistant" && message.webResearch}
      <span class="research-badge" title="This answer includes live web research">
        <span class="research-badge-dot" aria-hidden="true"></span>Web research
      </span>
    {/if}
    {#if isCompletedAssistant}
      <button
        class="copy-btn"
        onclick={copyMessage}
        title="Copy this answer as Markdown, with its sources"
      >
        {copied ? "Copied" : "Copy"}
      </button>
    {/if}
  </div>

  {#if showThinking}
    <div class="msg-content thinking-content" role="status" aria-live="polite">
      <span class="thinking-dot" class:animate={animateCursor} aria-hidden="true"></span>
      <span>{thinkingLabel}</span>
    </div>
  {:else if message.role === "assistant"}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="msg-content markdown" onclick={handleRenderedClick}>
      {@html renderAssistantMarkdown(message.content)}
      {#if streaming}<span
        class="type-cursor"
        class:animate={animateCursor}
        aria-hidden="true"
      ></span>{/if}
    </div>
  {:else}
    <div class="msg-content">
      {message.content}{#if streaming}<span
          class="type-cursor"
          class:animate={animateCursor}
          aria-hidden="true"
        ></span>{/if}
    </div>
  {/if}

  {#if message.role === "assistant" && message.sources && message.sources.length > 0}
    <div class="msg-sources">
      {#each message.sources as source}
        <button
          class="source-chip"
          onclick={() => onOpenSource(source)}
          title={`Open ${formatSourceLabel(source)}`}
        >
          <span class="source-index">[{source.index}]</span>
          <span class="source-title">{source.page_title}</span>
          {#if source.date}<span class="source-date">{source.date}</span>{/if}
        </button>
      {/each}
    </div>
  {/if}

  {#if message.role === "assistant" && message.webSources && message.webSources.length > 0}
    <div class="msg-sources web">
      {#each message.webSources as source}
        <button
          class="source-chip web-source-chip"
          onclick={() => onOpenWebSource(source)}
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
{/if}

<style>
  .msg {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 10px;
    background: var(--bg-primary);
  }

  .msg.user {
    border-color: var(--accent);
  }

  .msg.thinking {
    border-color: color-mix(in srgb, var(--accent-purple) 45%, var(--border));
    background: color-mix(in srgb, var(--accent-purple) 9%, var(--bg-primary));
  }

  .msg.thinking.tone-web {
    border-color: color-mix(in srgb, var(--accent-cyan) 45%, var(--border));
    background: color-mix(in srgb, var(--accent-cyan) 9%, var(--bg-primary));
  }

  .msg.thinking.tone-stalled {
    border-color: color-mix(in srgb, var(--accent-yellow) 55%, var(--border));
    background: color-mix(in srgb, var(--accent-yellow) 10%, var(--bg-primary));
  }

  .msg.thinking.tone-working {
    border-color: color-mix(in srgb, var(--accent) 45%, var(--border));
    background: color-mix(in srgb, var(--accent) 8%, var(--bg-primary));
  }

  .msg-role {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--text-muted);
    margin-bottom: 4px;
  }

  .msg-content {
    white-space: pre-wrap;
    word-break: break-word;
    line-height: 1.45;
  }

  .thinking-content {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--accent-purple);
    font-size: 13px;
    font-weight: 600;
  }

  .tone-web .thinking-content {
    color: var(--accent-cyan);
  }

  .tone-stalled .thinking-content {
    color: var(--accent-yellow);
  }

  .tone-working .thinking-content {
    color: var(--accent);
  }

  .thinking-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: currentColor;
    opacity: 0.9;
    flex-shrink: 0;
  }

  .thinking-dot.animate {
    animation: chat-pulse 1.2s ease-in-out infinite;
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

  .copy-btn {
    margin-left: auto;
    padding: 1px 8px;
    font-size: 10px;
    border: 1px solid var(--border-color, #333);
    border-radius: 5px;
    background: transparent;
    color: var(--text-muted, #888);
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.12s ease;
  }

  .msg:hover .copy-btn,
  .copy-btn:focus-visible {
    opacity: 1;
  }

  .copy-btn:hover {
    color: var(--text-primary, #eee);
    border-color: var(--text-muted, #777);
  }

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

  @keyframes chat-pulse {
    0%,
    100% {
      opacity: 0.3;
    }
    50% {
      opacity: 0.9;
    }
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

  @media (prefers-reduced-motion: reduce) {
    .thinking-dot.animate,
    .type-cursor.animate {
      animation: none;
    }
  }
</style>
