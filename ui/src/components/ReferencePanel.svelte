<script lang="ts">
  import {
    aiGenerateReferences,
    aiSearch,
    aiHealthCheck,
    aiAsk,
    type GeneratedReference,
    type PageReferencesMeta,
    type SemanticSearchResult,
    type HealthStatus,
  } from "../lib/knowledge";
  import type { PageNavigationTarget } from "../lib/navigation";

  // Props
  let {
    visible = false,
    pageId = "",
    pageTitle = "",
    onClose = () => {},
    onNavigate = (_target: PageNavigationTarget) => {},
  }: {
    visible?: boolean;
    pageId?: string;
    pageTitle?: string;
    onClose?: () => void;
    onNavigate?: (target: PageNavigationTarget) => void;
  } = $props();

  // State
  let activeTab = $state<"references" | "search" | "ask">("references");
  let references = $state<PageReferencesMeta | null>(null);
  let searchQuery = $state("");
  let searchResults = $state<SemanticSearchResult[]>([]);
  let askQuery = $state("");
  let askAnswer = $state("");
  let isLoading = $state(false);
  let error = $state("");
  let health = $state<HealthStatus | null>(null);

  // Check AI health on mount
  $effect(() => {
    if (visible) {
      aiHealthCheck().then((h) => (health = h)).catch(() => {});
    }
  });

  // ─── References ──────────────────────────────────────────────────────────────

  async function generateReferences() {
    if (!pageId) return;
    isLoading = true;
    error = "";
    try {
      references = await aiGenerateReferences(pageId);
    } catch (e: any) {
      error = e?.toString() || "Failed to generate references";
    } finally {
      isLoading = false;
    }
  }

  // ─── Search ────────────────────────────────────────────────────────────────

  async function doSearch() {
    if (!searchQuery.trim()) return;
    isLoading = true;
    error = "";
    try {
      searchResults = await aiSearch(searchQuery, 20);
    } catch (e: any) {
      error = e?.toString() || "Search failed";
    } finally {
      isLoading = false;
    }
  }

  // ─── Ask ───────────────────────────────────────────────────────────────────

  async function doAsk() {
    if (!askQuery.trim()) return;
    isLoading = true;
    error = "";
    askAnswer = "";
    try {
      askAnswer = await aiAsk(askQuery);
    } catch (e: any) {
      error = e?.toString() || "Ask failed";
    } finally {
      isLoading = false;
    }
  }

  function formatScore(score: number): string {
    return `${Math.round(score * 100)}%`;
  }

  function formatAge(timestamp: number): string {
    const days = Math.floor((Date.now() - timestamp) / (1000 * 60 * 60 * 24));
    if (days === 0) return "today";
    if (days === 1) return "yesterday";
    return `${days}d ago`;
  }
</script>

{#if visible}
  <aside class="reference-panel" class:panel-visible={visible}>
    <!-- Header -->
    <div class="panel-header">
      <div class="panel-tabs">
        <button
          class="tab-btn"
          class:active={activeTab === "references"}
          onclick={() => (activeTab = "references")}
        >
          References
        </button>
        <button
          class="tab-btn"
          class:active={activeTab === "search"}
          onclick={() => (activeTab = "search")}
        >
          Search
        </button>
        <button
          class="tab-btn"
          class:active={activeTab === "ask"}
          onclick={() => (activeTab = "ask")}
        >
          Ask
        </button>
      </div>
      <button class="close-btn" onclick={onClose} title="Close panel">✕</button>
    </div>

    <!-- Content -->
    <div class="panel-content">
      {#if !health?.enabled}
        <div class="panel-notice">
          <p>AI is not configured.</p>
          <p class="notice-sub">Go to Settings → AI to set up a provider.</p>
        </div>
      {:else if !health?.llm_available && !health?.embedder_available}
        <div class="panel-notice warning">
          <p>AI providers not reachable.</p>
          <p class="notice-sub">Check that Ollama is running or API keys are valid.</p>
        </div>
      {:else}
        <!-- References Tab -->
        {#if activeTab === "references"}
          <div class="tab-content">
            <div class="tab-actions">
              <button
                class="action-btn"
                onclick={generateReferences}
                disabled={isLoading || !pageId}
              >
                {isLoading ? "Analyzing..." : "Research this page"}
              </button>
              {#if health}
                <span class="vector-count">{health.vector_count} vectors indexed</span>
              {/if}
            </div>

            {#if error}
              <div class="error-msg">{error}</div>
            {/if}

            {#if references}
              <div class="refs-meta">
                Generated {formatAge(references.generated_at)} ·
                {references.reference_count} references found
              </div>

              {#each references.references as ref}
                <div class="reference-card">
                  <div class="ref-header">
                    <span class="ref-number">[{ref.ref_number}]</span>
                    <span class="ref-anchor">"{ref.anchor_text}"</span>
                    <span class="ref-confidence" title="Confidence">
                      {formatScore(ref.confidence)}
                    </span>
                  </div>
                  <div class="ref-body">
                    {#each ref.related_pages as related}
                      <button
                        class="related-page"
                        onclick={() => onNavigate({ id: related.page_id })}
                      >
                        <span class="related-title">{related.page_title}</span>
                        <span class="related-score">{formatScore(related.score)}</span>
                        <span class="related-snippet">{related.snippet}</span>
                      </button>
                    {/each}
                  </div>
                </div>
              {/each}

              {#if references.references.length === 0}
                <div class="panel-notice">No cross-references found for this page.</div>
              {/if}
            {:else if !isLoading}
              <div class="panel-notice">
                Click "Research this page" to discover connections.
              </div>
            {/if}
          </div>

        <!-- Search Tab -->
        {:else if activeTab === "search"}
          <div class="tab-content">
            <form class="search-form" onsubmit={(e) => { e.preventDefault(); doSearch(); }}>
              <input
                type="text"
                bind:value={searchQuery}
                placeholder="Semantic search across all graphs..."
                class="search-input"
              />
              <button type="submit" class="action-btn" disabled={isLoading}>
                {isLoading ? "..." : "Search"}
              </button>
            </form>

            {#if error}
              <div class="error-msg">{error}</div>
            {/if}

            {#each searchResults as result}
              <button
                class="search-result"
                onclick={() => onNavigate({ id: result.page_id })}
              >
                <div class="result-header">
                  <span class="result-title">{result.page_title}</span>
                  <span class="result-score">{formatScore(result.score)}</span>
                </div>
                <div class="result-snippet">{result.content.slice(0, 200)}</div>
              </button>
            {/each}
          </div>

        <!-- Ask Tab -->
        {:else if activeTab === "ask"}
          <div class="tab-content">
            <form class="search-form" onsubmit={(e) => { e.preventDefault(); doAsk(); }}>
              <input
                type="text"
                bind:value={askQuery}
                placeholder="Ask a question about your knowledge..."
                class="search-input"
              />
              <button type="submit" class="action-btn" disabled={isLoading}>
                {isLoading ? "Thinking..." : "Ask"}
              </button>
            </form>

            {#if error}
              <div class="error-msg">{error}</div>
            {/if}

            {#if askAnswer}
              <div class="ask-answer">
                {askAnswer}
              </div>
            {/if}
          </div>
        {/if}
      {/if}
    </div>
  </aside>
{/if}

<style>
  .reference-panel {
    position: fixed;
    top: 0;
    right: 0;
    bottom: 0;
    width: 380px;
    max-width: 90vw;
    background: var(--bg-secondary, #1e1e2e);
    border-left: 1px solid var(--border-color, #333);
    display: flex;
    flex-direction: column;
    z-index: 1000;
    overflow: hidden;
  }

  .panel-visible {
    /* kept for selector compatibility */
  }

  .panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border-color, #333);
    flex-shrink: 0;
  }

  .panel-tabs {
    display: flex;
    gap: 2px;
  }

  .tab-btn {
    background: none;
    border: none;
    color: var(--text-muted, #888);
    padding: 6px 12px;
    border-radius: 4px;
    cursor: pointer;
    font-size: 13px;
    transition: background 0.15s, color 0.15s;
  }

  .tab-btn:hover {
    background: var(--bg-hover, #2a2a3e);
  }

  .tab-btn.active {
    color: var(--text-primary, #fff);
    background: var(--bg-active, #333);
  }

  .close-btn {
    background: none;
    border: none;
    color: var(--text-muted, #888);
    font-size: 18px;
    cursor: pointer;
    padding: 4px 8px;
    border-radius: 4px;
  }

  .close-btn:hover {
    background: var(--bg-hover, #2a2a3e);
    color: var(--text-primary, #fff);
  }

  .panel-content {
    flex: 1;
    overflow-y: auto;
    padding: 12px;
  }

  .panel-notice {
    text-align: center;
    color: var(--text-muted, #888);
    padding: 24px 12px;
    font-size: 14px;
  }

  .panel-notice.warning {
    color: var(--warning-color, #f0a020);
  }

  .notice-sub {
    font-size: 12px;
    margin-top: 4px;
    opacity: 0.7;
  }

  .tab-content {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .tab-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .action-btn {
    background: var(--accent-color, #7c3aed);
    color: white;
    border: none;
    padding: 8px 16px;
    border-radius: 6px;
    cursor: pointer;
    font-size: 13px;
    white-space: nowrap;
  }

  .action-btn:hover:not(:disabled) {
    opacity: 0.9;
  }

  .action-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .vector-count {
    font-size: 11px;
    color: var(--text-muted, #888);
  }

  .error-msg {
    background: rgba(220, 38, 38, 0.1);
    border: 1px solid rgba(220, 38, 38, 0.3);
    color: #f87171;
    padding: 8px 12px;
    border-radius: 6px;
    font-size: 12px;
  }

  .refs-meta {
    font-size: 11px;
    color: var(--text-muted, #888);
  }

  .reference-card {
    background: var(--bg-tertiary, #252535);
    border: 1px solid var(--border-color, #333);
    border-radius: 8px;
    padding: 10px;
  }

  .ref-header {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 6px;
  }

  .ref-number {
    font-weight: bold;
    color: var(--accent-color, #7c3aed);
    font-size: 12px;
  }

  .ref-anchor {
    font-style: italic;
    color: var(--text-secondary, #aaa);
    font-size: 13px;
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ref-confidence {
    font-size: 11px;
    color: var(--text-muted, #888);
    background: var(--bg-secondary, #1e1e2e);
    padding: 2px 6px;
    border-radius: 4px;
  }

  .ref-body {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .related-page {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    background: var(--bg-secondary, #1e1e2e);
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 6px 8px;
    cursor: pointer;
    text-align: left;
    width: 100%;
    transition: border-color 0.15s;
  }

  .related-page:hover {
    border-color: var(--accent-color, #7c3aed);
  }

  .related-title {
    font-size: 13px;
    color: var(--text-primary, #fff);
    font-weight: 500;
  }

  .related-score {
    font-size: 10px;
    color: var(--text-muted, #888);
  }

  .related-snippet {
    font-size: 11px;
    color: var(--text-secondary, #aaa);
    margin-top: 2px;
    line-height: 1.3;
  }

  .search-form {
    display: flex;
    gap: 8px;
  }

  .search-input {
    flex: 1;
    background: var(--bg-tertiary, #252535);
    border: 1px solid var(--border-color, #333);
    color: var(--text-primary, #fff);
    padding: 8px 12px;
    border-radius: 6px;
    font-size: 13px;
    outline: none;
  }

  .search-input:focus {
    border-color: var(--accent-color, #7c3aed);
  }

  .search-result {
    display: flex;
    flex-direction: column;
    background: var(--bg-tertiary, #252535);
    border: 1px solid var(--border-color, #333);
    border-radius: 8px;
    padding: 10px;
    cursor: pointer;
    text-align: left;
    width: 100%;
    transition: border-color 0.15s;
  }

  .search-result:hover {
    border-color: var(--accent-color, #7c3aed);
  }

  .result-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 4px;
  }

  .result-title {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-primary, #fff);
  }

  .result-score {
    font-size: 11px;
    color: var(--text-muted, #888);
  }

  .result-snippet {
    font-size: 12px;
    color: var(--text-secondary, #aaa);
    line-height: 1.4;
  }

  .ask-answer {
    background: var(--bg-tertiary, #252535);
    border: 1px solid var(--border-color, #333);
    border-radius: 8px;
    padding: 12px;
    font-size: 13px;
    color: var(--text-primary, #fff);
    line-height: 1.6;
    white-space: pre-wrap;
  }

  /* Mobile adjustments */
  @media (max-width: 768px) {
    .reference-panel {
      width: 100%;
      max-width: 100%;
      top: auto;
      height: 70vh;
      border-left: none;
      border-top: 1px solid var(--border-color, #333);
      border-radius: 16px 16px 0 0;
      transform: translateY(100%);
    }

    .panel-visible {
      transform: translateY(0);
    }
  }
</style>
