<script lang="ts">
  import { tick } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { open as openExternal } from "@tauri-apps/plugin-shell";
  import {
    aiGenerateReferences,
    aiSearch,
    aiHealthCheck,
    aiAsk,
    aiInsertPageSummary,
    aiSummarizeSelection,
    aiResearchWeb,
    type GeneratedReference,
    type PageReferencesMeta,
    type PageSummary,
    type WebResearchResult,
    type SemanticSearchResult,
    type HealthStatus,
  } from "../lib/knowledge";
  import type { PageNavigationTarget } from "../lib/navigation";

  // Props
  let {
    visible = false,
    pageId = "",
    pageTitle = "",
    initialTab,
    focusTrigger = 0,
    onClose = () => {},
    onNavigate = (_target: PageNavigationTarget) => {},
  }: {
    visible?: boolean;
    pageId?: string;
    pageTitle?: string;
    initialTab?: "references" | "search" | "ask";
    focusTrigger?: number;
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
  let researchProgress = $state("");
  let isInsertingSummary = $state(false);
  let insertedSummary = $state(false);
  let searchInputEl = $state<HTMLInputElement | null>(null);

  // Jump to (and focus) the requested tab whenever the parent bumps focusTrigger,
  // e.g. from the toolbar "Search" button or the global Ctrl+K shortcut.
  $effect(() => {
    // eslint-disable-next-line @typescript-eslint/no-unused-expressions
    focusTrigger;
    if (!initialTab) return;
    activeTab = initialTab;
    if (initialTab === "search") {
      tick().then(() => searchInputEl?.focus());
    }
  });

  // ─── Analyze Selection (arbitrary drag-selected text, not block-select) ──────
  let hasTextSelection = $state(false);
  let pendingSelectionText = $state("");
  let selectionSummary = $state<PageSummary | null>(null);
  let isAnalyzingSelection = $state(false);
  let selectionError = $state("");
  let selectionProgress = $state("");
  let isInsertingSelectionSummary = $state(false);
  let insertedSelectionSummary = $state(false);

  // ─── Web Research (real internet search + cited synthesis) ──────────────────
  let webResearchResult = $state<WebResearchResult | null>(null);
  let isResearchingWeb = $state(false);
  let webResearchError = $state("");
  let webResearchProgress = $state("");
  let isInsertingWebResearch = $state(false);
  let insertedWebResearch = $state(false);

  async function researchWeb() {
    if (!pageId || isResearchingWeb) return;
    isResearchingWeb = true;
    webResearchError = "";
    insertedWebResearch = false;
    webResearchResult = null;
    webResearchProgress = "Starting web research...";
    const unlisten = await listen<string>("ai-web-research-progress", (e) => {
      webResearchProgress = e.payload;
    });
    try {
      // Selected/visible page content isn't available here directly, so
      // the seed text is just the title — the LLM plans search queries
      // from the title alone, same as a user typing a topic into a search
      // engine themselves.
      webResearchResult = await aiResearchWeb(pageTitle, pageTitle);
    } catch (e: any) {
      webResearchError = e?.toString() || "Web research failed";
    } finally {
      unlisten();
      isResearchingWeb = false;
      webResearchProgress = "";
    }
  }

  async function insertWebResearchIntoPage() {
    if (!pageId || !webResearchResult || isInsertingWebResearch) return;
    isInsertingWebResearch = true;
    webResearchError = "";
    try {
      // Append a numbered "Sources" list after each topic paragraph so the
      // inline [n] markers the AI wrote stay meaningful once inserted into
      // the page, same shape as the on-screen citation list below.
      const sourcesList = webResearchResult.citations
        .map((c) => `${c.number}. [${c.title}](${c.url})`)
        .join("\n");
      const topicsWithSources = webResearchResult.topics.map((t, i) => ({
        ...t,
        summary:
          i === webResearchResult!.topics.length - 1 && sourcesList
            ? `${t.summary}\n\n**Sources:**\n${sourcesList}`
            : t.summary,
      }));
      await writeSummaryIntoPage({
        title_answer: webResearchResult.title_answer,
        topics: topicsWithSources,
      });
      insertedWebResearch = true;
    } catch (e: any) {
      webResearchError = e?.toString() || "Failed to insert research into page";
    } finally {
      isInsertingWebResearch = false;
    }
  }

  function openCitation(url: string) {
    openExternal(url).catch(() => {});
  }

  // Tracks whether the browser currently has a non-empty text selection
  // anywhere on the page (e.g. the user dragged over prose inside a
  // block), so the "Analyze Selection" button can enable/disable itself
  // without requiring the bullet-click block-selection mechanism.
  $effect(() => {
    if (!visible) return;
    const update = () => {
      hasTextSelection = (window.getSelection()?.toString().trim().length ?? 0) > 0;
    };
    update();
    document.addEventListener("selectionchange", update);
    return () => document.removeEventListener("selectionchange", update);
  });

  // Clicking the button would normally collapse the text selection before
  // onclick fires (mousedown resets it) — preventing that on mousedown is
  // the standard trick to let a toolbar button act on an existing text
  // selection instead of stealing it.
  function captureSelectionOnMouseDown(e: MouseEvent) {
    e.preventDefault();
    pendingSelectionText = window.getSelection()?.toString() ?? "";
  }

  async function analyzeSelection() {
    const text = pendingSelectionText.trim();
    if (!text || isAnalyzingSelection) return;
    isAnalyzingSelection = true;
    selectionError = "";
    insertedSelectionSummary = false;
    selectionProgress = "Analyzing selection...";
    const unlisten = await listen<string>("ai-selection-summary-progress", (e) => {
      selectionProgress = e.payload;
    });
    try {
      selectionSummary = await aiSummarizeSelection(text, pageTitle);
    } catch (e: any) {
      selectionError = e?.toString() || "Failed to analyze selection";
    } finally {
      unlisten();
      isAnalyzingSelection = false;
      selectionProgress = "";
    }
  }

  /// Shared by both the whole-page summary and the selection summary:
  /// writes title-answer + one heading/paragraph per topic as a new block
  /// at the top of the page, and wraps each topic's tags in place across
  /// the page's existing blocks.
  async function writeSummaryIntoPage(summary: PageSummary): Promise<void> {
    await aiInsertPageSummary(pageId, summary.title_answer, summary.topics ?? []);
    window.dispatchEvent(new CustomEvent("page-content-reload-blocks", { detail: { pageId } }));
  }

  async function insertSelectionSummaryIntoPage() {
    if (!pageId || !selectionSummary || isInsertingSelectionSummary) return;
    isInsertingSelectionSummary = true;
    selectionError = "";
    try {
      await writeSummaryIntoPage(selectionSummary);
      insertedSelectionSummary = true;
    } catch (e: any) {
      selectionError = e?.toString() || "Failed to insert summary into page";
    } finally {
      isInsertingSelectionSummary = false;
    }
  }

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
    insertedSummary = false;
    researchProgress = "Starting analysis...";
    const unlisten = await listen<string>("ai-reference-progress", (e) => {
      // Cap what we keep in memory/DOM — streamed model output can grow
      // unbounded over a multi-minute generation, but only the tail is
      // useful to show as a "live" indicator.
      const MAX_PROGRESS_CHARS = 400;
      const text = e.payload;
      researchProgress =
        text.length > MAX_PROGRESS_CHARS ? "…" + text.slice(-MAX_PROGRESS_CHARS) : text;
    });
    try {
      references = await aiGenerateReferences(pageId);
    } catch (e: any) {
      error = e?.toString() || "Failed to generate references";
    } finally {
      unlisten();
      isLoading = false;
      researchProgress = "";
    }
  }

  /// Writes the current summary into the actual page: a new block right
  /// after the title (title-answer + one heading/paragraph per topic),
  /// plus each topic's tags wrapped in place as `[[wiki-link]]`s across
  /// the page's existing blocks. Explicit opt-in button rather than
  /// automatic on every "Research this page" run, so re-running research
  /// never silently duplicates content on the page.
  async function insertSummaryIntoPage() {
    if (!pageId || !references?.summary || isInsertingSummary) return;
    isInsertingSummary = true;
    error = "";
    try {
      await writeSummaryIntoPage(references.summary);
      insertedSummary = true;
    } catch (e: any) {
      error = e?.toString() || "Failed to insert summary into page";
    } finally {
      isInsertingSummary = false;
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
                {isLoading ? "Analyzing..." : "Analyze this Page"}
              </button>
              <button
                class="action-btn"
                onmousedown={captureSelectionOnMouseDown}
                onclick={analyzeSelection}
                disabled={isAnalyzingSelection || !hasTextSelection || !pageId}
                title={hasTextSelection ? "Summarize the highlighted text" : "Highlight some text on the page first"}
              >
                {isAnalyzingSelection ? "Analyzing selection..." : "Analyze Selection"}
              </button>
              <button
                class="action-btn"
                onclick={researchWeb}
                disabled={isResearchingWeb || !pageId}
                title="Search the internet for this topic and write a cited summary with clickable sources"
              >
                {isResearchingWeb ? "Researching..." : "Web Research"}
              </button>
              {#if health}
                <span class="vector-count">{health.vector_count} vectors indexed</span>
              {/if}
            </div>

            {#if error}
              <div class="error-msg">{error}</div>
            {/if}

            {#if isLoading && researchProgress}
              <div class="progress-status">
                <span class="progress-spinner"></span>
                <span class="progress-text">{researchProgress}</span>
              </div>
            {/if}

            {#if selectionError}
              <div class="error-msg">{selectionError}</div>
            {/if}

            {#if isAnalyzingSelection && selectionProgress}
              <div class="progress-status">
                <span class="progress-spinner"></span>
                <span class="progress-text">{selectionProgress}</span>
              </div>
            {/if}

            {#if selectionSummary}
              <div class="summary-card">
                <div class="summary-card-label">Selection summary</div>
                {#if selectionSummary.title_answer}
                  <div class="summary-title-answer">{selectionSummary.title_answer}</div>
                {/if}
                {#each selectionSummary.topics as topic}
                  <div class="summary-topic">
                    <div class="summary-topic-title">{topic.topic}</div>
                    <div class="summary-text">{topic.summary}</div>
                    {#if topic.tags?.length}
                      <div class="summary-tags">
                        {#each topic.tags as tag}
                          <span class="summary-tag">#{tag.qualified ?? tag.term}</span>
                        {/each}
                      </div>
                    {/if}
                  </div>
                {/each}
                <button
                  class="insert-summary-btn"
                  onclick={insertSelectionSummaryIntoPage}
                  disabled={isInsertingSelectionSummary || insertedSelectionSummary}
                >
                  {#if insertedSelectionSummary}
                    Inserted into page ✓
                  {:else if isInsertingSelectionSummary}
                    Inserting...
                  {:else}
                    Insert into page
                  {/if}
                </button>
              </div>
            {/if}

            {#if webResearchError}
              <div class="error-msg">{webResearchError}</div>
            {/if}

            {#if isResearchingWeb && webResearchProgress}
              <div class="progress-status">
                <span class="progress-spinner"></span>
                <span class="progress-text">{webResearchProgress}</span>
              </div>
            {/if}

            {#if webResearchResult}
              <div class="summary-card">
                <div class="summary-card-label">Web research</div>
                {#if webResearchResult.title_answer}
                  <div class="summary-title-answer">{webResearchResult.title_answer}</div>
                {/if}
                {#each webResearchResult.topics as topic}
                  <div class="summary-topic">
                    <div class="summary-topic-title">{topic.topic}</div>
                    <div class="summary-text">{topic.summary}</div>
                    {#if topic.tags?.length}
                      <div class="summary-tags">
                        {#each topic.tags as tag}
                          <span class="summary-tag">#{tag.qualified ?? tag.term}</span>
                        {/each}
                      </div>
                    {/if}
                  </div>
                {/each}
                {#if webResearchResult.citations.length}
                  <div class="citations-list">
                    <div class="citations-label">Sources</div>
                    {#each webResearchResult.citations as citation}
                      <button
                        class="citation-link"
                        onclick={() => openCitation(citation.url)}
                        title={citation.url}
                      >
                        [{citation.number}] {citation.title}
                      </button>
                    {/each}
                  </div>
                {/if}
                <button
                  class="insert-summary-btn"
                  onclick={insertWebResearchIntoPage}
                  disabled={isInsertingWebResearch || insertedWebResearch}
                >
                  {#if insertedWebResearch}
                    Inserted into page ✓
                  {:else if isInsertingWebResearch}
                    Inserting...
                  {:else}
                    Insert into page
                  {/if}
                </button>
              </div>
            {/if}

            {#if references?.summary}
              <div class="summary-card">
                {#if references.summary.title_answer}
                  <div class="summary-title-answer">{references.summary.title_answer}</div>
                {/if}
                {#each references.summary.topics as topic}
                  <div class="summary-topic">
                    <div class="summary-topic-title">{topic.topic}</div>
                    <div class="summary-text">{topic.summary}</div>
                    {#if topic.tags?.length}
                      <div class="summary-tags">
                        {#each topic.tags as tag}
                          <span class="summary-tag">#{tag.qualified ?? tag.term}</span>
                        {/each}
                      </div>
                    {/if}
                  </div>
                {/each}
                <button
                  class="insert-summary-btn"
                  onclick={insertSummaryIntoPage}
                  disabled={isInsertingSummary || insertedSummary}
                >
                  {#if insertedSummary}
                    Inserted into page ✓
                  {:else if isInsertingSummary}
                    Inserting...
                  {:else}
                    Insert into page
                  {/if}
                </button>
              </div>
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
                Click "Analyze this Page" to discover connections.
              </div>
            {/if}
          </div>

        <!-- Search Tab -->
        {:else if activeTab === "search"}
          <div class="tab-content">
            <form class="search-form" onsubmit={(e) => { e.preventDefault(); doSearch(); }}>
              <input
                type="text"
                bind:this={searchInputEl}
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
    flex-wrap: wrap;
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

  .progress-status {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    background: var(--bg-tertiary, #252535);
    border: 1px solid var(--border-color, #333);
    border-radius: 6px;
    padding: 8px 12px;
    font-size: 12px;
    color: var(--text-secondary, #aaa);
  }

  .progress-spinner {
    width: 12px;
    height: 12px;
    flex-shrink: 0;
    margin-top: 2px;
    border: 2px solid var(--border-color, #444);
    border-top-color: var(--accent-color, #7c3aed);
    border-radius: 50%;
    animation: progress-spin 0.8s linear infinite;
  }

  .progress-text {
    overflow-wrap: anywhere;
    white-space: pre-wrap;
    max-height: 6.4em;
    overflow-y: auto;
    font-family: var(--mono-font, monospace);
    line-height: 1.4;
  }

  @keyframes progress-spin {
    to {
      transform: rotate(360deg);
    }
  }

  .summary-card {
    background: var(--bg-tertiary, #252535);
    border: 1px solid var(--accent-color, #7c3aed);
    border-radius: 8px;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .summary-card-label {
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-muted, #888);
  }

  .summary-title-answer {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary, #fff);
    line-height: 1.4;
  }

  .summary-topic {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .summary-topic + .summary-topic {
    padding-top: 8px;
    border-top: 1px solid var(--bg-secondary, #1a1a24);
  }

  .summary-topic-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--accent-color, #7c3aed);
  }

  .summary-text {
    font-size: 12px;
    color: var(--text-secondary, #aaa);
    line-height: 1.5;
  }

  .summary-tags {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .summary-tag {
    font-size: 11px;
    color: var(--accent-color, #7c3aed);
    background: var(--bg-secondary, #1a1a24);
    border-radius: 4px;
    padding: 2px 6px;
  }

  .citations-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 4px;
  }

  .citations-label {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.03em;
    color: var(--text-secondary, #888);
  }

  .citation-link {
    align-self: flex-start;
    font-size: 12px;
    text-align: left;
    color: var(--accent-color, #7c3aed);
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .citation-link:hover {
    color: var(--text-primary, #fff);
  }

  .insert-summary-btn {
    align-self: flex-start;
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-primary, #fff);
    background: var(--accent-color, #7c3aed);
    border: none;
    border-radius: 4px;
    padding: 5px 10px;
    cursor: pointer;
  }

  .insert-summary-btn:disabled {
    opacity: 0.6;
    cursor: default;
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
