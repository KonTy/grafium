<script lang="ts">
  import { tick } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { open as openExternal } from "@tauri-apps/plugin-shell";
  import ChatMessageBubble from "./ChatMessageBubble.svelte";
  import { searchFts, searchPageTitles, getPage, listBlocks, updateBlock, type Block, type PageSummary as ApiPageSummary } from "../lib/api";
  import {
    aiGenerateReferences,
    aiSearch,
    aiHealthCheck,
    aiAsk,
    aiInsertPageSummary,
    aiSummarizeSelection,
    aiResearchWeb,
    aiCancelOperation,
    type GeneratedReference,
    type PageReferencesMeta,
    type PageSummary,
    type WebResearchResult,
    type WebSource,
    type SemanticSearchResult,
    type HealthStatus,
  } from "../lib/knowledge";
  import type { ChatMessageModel, ChatThinkingTone } from "../lib/chatMessage";
  import { pushUndo } from "../lib/undoStack";
  import type { PageNavigationTarget } from "../lib/navigation";
  import {
    getCurrentBlockAnchor,
    getLatestCurrentBlockAnchor,
    type CurrentBlockAnchor,
  } from "../lib/currentBlockAnchor";

  // Props
  let {
    visible = false,
    pageId = "",
    pageTitle = "",
    initialTab,
    focusTrigger = 0,
    width = 380,
    preferFocusedPageForPageScope = false,
    onClose = () => {},
    onNavigate = (_target: PageNavigationTarget) => {},
    onFindLinks = (_page: { id: string; title: string }) => {},
  }: {
    visible?: boolean;
    pageId?: string;
    pageTitle?: string;
    initialTab?: "references" | "search" | "ask";
    focusTrigger?: number;
    width?: number;
    preferFocusedPageForPageScope?: boolean;
    onClose?: () => void;
    onNavigate?: (target: PageNavigationTarget) => void;
    onFindLinks?: (page: { id: string; title: string }) => void;
  } = $props();

  // State
  let activeTab = $state<"references" | "search" | "ask">("references");
  let references = $state<PageReferencesMeta | null>(null);
  let searchQuery = $state("");
  let searchResults = $state<SemanticSearchResult[]>([]);
  type QuickMatch =
    | { kind: "page"; page: ApiPageSummary }
    | { kind: "block"; block: Block; pageTitle: string };
  let quickMatches = $state<QuickMatch[]>([]);
  let isQuickSearching = $state(false);
  let selectedResultIndex = $state(-1);
  let askQuery = $state("");
  let askAnswer = $state("");
  let isLoading = $state(false);
  let error = $state("");
  let health = $state<HealthStatus | null>(null);
  let researchProgress = $state("");
  let isInsertingSummary = $state(false);
  let insertedSummary = $state(false);
  let searchInputEl = $state<HTMLInputElement | null>(null);
  let askWebResearchResult = $state<WebResearchResult | null>(null);
  let askWebResearchProgress = $state("");
  let askMessages = $state<ChatMessageModel[]>([]);
  let askPendingAssistantIndex = $state<number | null>(null);
  let askThreadGeneration = 0;
  let activeAskGeneration: number | null = null;
  let askThinkingLabel = $state("Thinking…");
  let askThinkingTone = $state<ChatThinkingTone>("thinking");
  let askScrollEl = $state<HTMLDivElement | null>(null);
  type AskTurn = {
    id: string;
    scope: AskScope;
    question: string;
    answer: string;
    pageTitle?: string;
    blockId?: string;
    webResearchResult?: WebResearchResult;
  };
  let askTurns = $state<AskTurn[]>([]);
  let askThreadSummary = $state("");
  let isSummarizingAskThread = $state(false);
  let improvedBlockDraft = $state("");
  let improvedBlockContext = $state<CurrentBlockContext | null>(null);
  let isImprovingBlock = $state(false);
  let isApplyingBlockDraft = $state(false);
  let blockDraftStatus = $state("");

  // Operation IDs for the three cancellable AI operations. Set to a fresh
  // UUID for the duration of an in-flight run so the Cancel button can
  // send it back to the backend, cleared to null when the run ends. We
  // key state per-operation so the three progress toasts on this panel
  // can be cancelled independently (they *can* run in parallel, though
  // the local LLM worker serializes them anyway).
  let analyzePageOpId = $state<string | null>(null);
  let analyzeSelectionOpId = $state<string | null>(null);
  let webResearchOpId = $state<string | null>(null);
  // Track which operations the user has already asked to cancel so the
  // button can go disabled/relabelled and we don't spam the backend with
  // duplicate cancels. Reset each new run.
  let analyzePageCancelling = $state(false);
  let analyzeSelectionCancelling = $state(false);
  let webResearchCancelling = $state(false);

  // Cheap unique-per-operation ID. `crypto.randomUUID()` is available in
  // WebViews everywhere Tauri runs today (Chromium ≥92, WebView2 recent,
  // WebKit ≥15.4), and even if it weren't, the backend only compares the
  // string for equality so any unique-ish value works.
  function newOpId(): string {
    try {
      return crypto.randomUUID();
    } catch {
      return `op-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
    }
  }

  // Jump to the requested tab whenever the parent bumps focusTrigger, e.g.
  // from the toolbar "Search" button or the global Ctrl+K shortcut.
  $effect(() => {
    // eslint-disable-next-line @typescript-eslint/no-unused-expressions
    focusTrigger;
    if (!initialTab) return;
    activeTab = initialTab;
  });

  // Focus the search input whenever the Search tab becomes active -- as its
  // own effect keyed on activeTab (rather than bundled into the effect above)
  // so it reliably re-fires once the tab's DOM (and the input) actually
  // exists, including on the very first Ctrl+K press that both opens the
  // panel and switches to this tab in the same update.
  $effect(() => {
    if (activeTab !== "search") return;
    tick().then(() => tick()).then(() => searchInputEl?.focus());
  });

  // ─── Cursor anchor (for "Insert into page" positioning) ─────────────────────
  // Id of the block the user had a caret in most recently on the currently
  // visible page. PageContent.svelte broadcasts `page-content-focus-changed`
  // whenever focus enters a block, and again with blockId=null whenever the
  // visible page changes. We remember the id across blur (unlike PageContent's
  // own `focusedBlockId`) because clicking the "Insert into page" button
  // itself steals focus from the block, so by the time our onclick runs the
  // editor no longer has an active caret to query. Anchored inserts fall
  // back to top-of-page (in the Rust command) if the id is null or stale.
  let lastFocusedBlockId = $state<string | null>(null);
  let askBlockAnchor = $state<CurrentBlockAnchor | null>(null);

  $effect(() => {
    const handleFocusChanged = (event: Event) => {
      const detail = (event as CustomEvent<{ pageId: string; blockId: string | null }>).detail;
      if (!detail) return;
      if (detail.blockId) {
        askBlockAnchor = { pageId: detail.pageId, blockId: detail.blockId };
      } else if (askBlockAnchor?.pageId === detail.pageId) {
        askBlockAnchor = null;
      }
      // Summary insertion still anchors only within the panel's current page.
      if (detail.pageId === pageId) {
        lastFocusedBlockId = detail.blockId;
      }
    };
    window.addEventListener("page-content-focus-changed", handleFocusChanged);
    return () => window.removeEventListener("page-content-focus-changed", handleFocusChanged);
  });

  // Restore the last clicked/focused block when the panel mounts after the
  // user picked a block while the right panel was closed.
  $effect(() => {
    // eslint-disable-next-line @typescript-eslint/no-unused-expressions
    pageId;
    askBlockAnchor = getLatestCurrentBlockAnchor();
    lastFocusedBlockId = pageId ? getCurrentBlockAnchor(pageId) : null;
  });

  // ─── Summarize Selection (arbitrary drag-selected text, not block-select) ────
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
    const opId = newOpId();
    webResearchOpId = opId;
    webResearchCancelling = false;
    const unlisten = await listen<string>("ai-web-research-progress", (e) => {
      webResearchProgress = e.payload;
    });
    try {
      // Selected/visible page content isn't available here directly, so
      // the seed text is just the title — the LLM plans search queries
      // from the title alone, same as a user typing a topic into a search
      // engine themselves.
      webResearchResult = await aiResearchWeb(pageTitle, pageTitle, opId);
    } catch (e: any) {
      webResearchError = e?.toString() || "Web research failed";
    } finally {
      unlisten();
      isResearchingWeb = false;
      webResearchProgress = "";
      webResearchOpId = null;
      webResearchCancelling = false;
    }
  }

  // Signal the backend to stop whichever operation just had its Cancel
  // button pressed. Idempotent: safe to call even if the operation just
  // finished before the click landed. We deliberately don't wait for the
  // backend to acknowledge — the running invoke() promise resolves with
  // an error on its own once the cancel token flips, so the finally
  // block in the parent function is what actually clears UI state.
  async function cancelOperation(
    kind: "analyze-page" | "analyze-selection" | "web-research"
  ) {
    let opId: string | null = null;
    if (kind === "analyze-page") {
      if (!analyzePageOpId || analyzePageCancelling) return;
      opId = analyzePageOpId;
      analyzePageCancelling = true;
    } else if (kind === "analyze-selection") {
      if (!analyzeSelectionOpId || analyzeSelectionCancelling) return;
      opId = analyzeSelectionOpId;
      analyzeSelectionCancelling = true;
    } else {
      if (!webResearchOpId || webResearchCancelling) return;
      opId = webResearchOpId;
      webResearchCancelling = true;
    }
    try {
      await aiCancelOperation(opId);
    } catch (e) {
      // Cancel failures are almost always "unknown operation id" from a
      // race where the op finished in the meantime — nothing actionable
      // for the user, so we just swallow rather than surface an error.
      console.warn("Cancel failed", e);
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

  function openChatWebSource(source: WebSource) {
    openCitation(source.url);
  }

  // Tracks whether the browser currently has a non-empty text selection
  // anywhere on the page (e.g. the user dragged over prose inside a
  // block), so the "Summarize Selection" button can enable/disable itself
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
    selectionProgress = "Summarizing selection...";
    const opId = newOpId();
    analyzeSelectionOpId = opId;
    analyzeSelectionCancelling = false;
    const unlisten = await listen<string>("ai-selection-summary-progress", (e) => {
      selectionProgress = e.payload;
    });
    try {
      selectionSummary = await aiSummarizeSelection(text, pageTitle, opId);
    } catch (e: any) {
      selectionError = e?.toString() || "Failed to summarize selection";
    } finally {
      unlisten();
      isAnalyzingSelection = false;
      selectionProgress = "";
      analyzeSelectionOpId = null;
      analyzeSelectionCancelling = false;
    }
  }

  /// Shared by both the whole-page summary and the selection summary:
  /// writes title-answer + one heading/paragraph per topic as a new block
  /// — anchored right after the block the user last had a caret in
  /// (`lastFocusedBlockId`) so the summary lands where they were reading,
  /// or at the top of the page as a fallback when there's no cursor to
  /// anchor to — and wraps each topic's tags in place across the page's
  /// existing blocks. Pushes a matching entry onto the app undo stack so
  /// Ctrl-Z reverses both the newly-inserted block AND every wrap edit
  /// this call caused, without touching other user edits.
  async function writeSummaryIntoPage(summary: PageSummary): Promise<void> {
    const result = await aiInsertPageSummary(
      pageId,
      summary.title_answer,
      summary.topics ?? [],
      lastFocusedBlockId,
    );
    pushUndo({
      type: "insert_summary",
      pageId,
      insertedBlockId: result.insertedBlockId,
      insertedContent: result.insertedContent,
      insertedAfterBlockId: result.insertedAfterBlockId,
      wrapChanges: result.wrapChanges,
    });
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
    const opId = newOpId();
    analyzePageOpId = opId;
    analyzePageCancelling = false;
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
      references = await aiGenerateReferences(pageId, opId);
    } catch (e: any) {
      error = e?.toString() || "Failed to generate references";
    } finally {
      unlisten();
      isLoading = false;
      researchProgress = "";
      analyzePageOpId = null;
      analyzePageCancelling = false;
    }
  }

  function findLinksForCurrentPage() {
    if (!pageId) return;
    onFindLinks({ id: pageId, title: pageTitle });
  }

  /// Writes the current summary into the actual page as a new block
  /// (title-answer + one heading/paragraph per topic), plus each topic's
  /// tags wrapped in place as `[[wiki-link]]`s across the page's existing
  /// blocks. When the user has an active caret in a block on the page,
  /// the summary is inserted immediately after that block so it lands
  /// where they were reading; otherwise it goes to the top of the page.
  /// Explicit opt-in button rather than automatic on every "Research
  /// this page" run, so re-running research never silently duplicates
  /// content on the page.
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

  const pageTitleCache = new Map<string, string>();
  let quickSearchVersion = 0;
  let quickSearchTimer: ReturnType<typeof setTimeout> | null = null;

  // Unified list backing keyboard navigation: quick (instant) matches first,
  // then AI semantic results, so Up/Down/Enter can move through everything
  // shown on screen regardless of which section it came from.
  type CombinedResult =
    | { source: "quick"; match: QuickMatch }
    | { source: "ai"; result: SemanticSearchResult };
  let combinedResults = $derived<CombinedResult[]>([
    ...quickMatches.map((match) => ({ source: "quick" as const, match })),
    ...searchResults.map((result) => ({ source: "ai" as const, result })),
  ]);

  function navigateToCombinedResult(entry: CombinedResult) {
    if (entry.source === "quick") {
      navigateToQuickMatch(entry.match);
    } else {
      onNavigate({ id: entry.result.page_id });
    }
  }

  // Instant, partial-word literal search (page titles + block content),
  // triggered on every keystroke -- like Logseq, typing "magn" finds
  // "magnesium" immediately without waiting for/needing an embedding model.
  // Extracted so both the live-as-you-type path (debounced, below) and the
  // explicit "Search" button (immediate, no debounce, no AI involved at
  // all) share one implementation -- this is the plain SQLite FTS path,
  // completely independent of any local/cloud LLM or embedder.
  async function runQuickSearch(trimmed: string, requestVersion: number) {
    try {
      const [pages, blocks] = await Promise.all([
        searchPageTitles(trimmed, 8),
        searchFts(trimmed, 15),
      ]);
      if (requestVersion !== quickSearchVersion) return;

      const blockMatches: QuickMatch[] = await Promise.all(
        blocks.slice(0, 12).map(async (block) => {
          let title = pageTitleCache.get(block.page_id);
          if (!title) {
            try {
              const page = await getPage({ id: block.page_id });
              title = page.title;
              pageTitleCache.set(block.page_id, title);
            } catch {
              title = "(unknown page)";
            }
          }
          return { kind: "block" as const, block, pageTitle: title };
        })
      );
      if (requestVersion !== quickSearchVersion) return;

      quickMatches = [
        ...pages.map((page) => ({ kind: "page" as const, page })),
        ...blockMatches,
      ];
    } catch {
      // Quick search is best-effort; ignore failures silently.
    } finally {
      if (requestVersion === quickSearchVersion) isQuickSearching = false;
    }
  }

  function handleSearchInput() {
    selectedResultIndex = -1;
    const trimmed = searchQuery.trim();
    quickSearchVersion += 1;
    const requestVersion = quickSearchVersion;
    if (quickSearchTimer) clearTimeout(quickSearchTimer);

    if (!trimmed) {
      quickMatches = [];
      isQuickSearching = false;
      return;
    }

    isQuickSearching = true;
    quickSearchTimer = setTimeout(() => runQuickSearch(trimmed, requestVersion), 120);
  }

  // Explicit, immediate plain-text search -- same SQLite FTS path as the
  // live-as-you-type quick matches above, just triggered right away (no
  // 120ms debounce) and with no AI/LLM/embedder involvement whatsoever.
  // This is the "Search" button placed next to "AI Search".
  function doPlainSearch() {
    const trimmed = searchQuery.trim();
    if (!trimmed) return;
    if (quickSearchTimer) clearTimeout(quickSearchTimer);
    selectedResultIndex = -1;
    quickSearchVersion += 1;
    isQuickSearching = true;
    void runQuickSearch(trimmed, quickSearchVersion);
  }

  // Arrow keys move a highlighted selection through the combined results
  // list without ever moving focus off the input, so typing always resumes
  // right where you left off (focus "snaps back" for free -- it never left).
  function handleSearchKeydown(e: KeyboardEvent) {
    if (e.key === "ArrowDown") {
      if (combinedResults.length === 0) return;
      e.preventDefault();
      selectedResultIndex = Math.min(selectedResultIndex + 1, combinedResults.length - 1);
    } else if (e.key === "ArrowUp") {
      if (combinedResults.length === 0) return;
      e.preventDefault();
      selectedResultIndex = Math.max(selectedResultIndex - 1, -1);
    } else if (e.key === "Enter") {
      if (selectedResultIndex >= 0 && selectedResultIndex < combinedResults.length) {
        e.preventDefault();
        navigateToCombinedResult(combinedResults[selectedResultIndex]);
      } else if (combinedResults.length > 0) {
        // Nothing explicitly highlighted yet -- Enter jumps to the top result.
        e.preventDefault();
        navigateToCombinedResult(combinedResults[0]);
      }
      // Otherwise let the form's onsubmit run the deeper AI search.
    } else if (e.key === "Escape") {
      selectedResultIndex = -1;
    }
  }

  function navigateToQuickMatch(match: QuickMatch) {
    if (match.kind === "page") {
      onNavigate({ id: match.page.id });
      return;
    }
    window.dispatchEvent(
      new CustomEvent("navigate-page", {
        detail: { pageName: match.pageTitle, targetBlockId: match.block.id },
      })
    );
  }

  async function doSearch() {
    if (!searchQuery.trim()) return;
    isLoading = true;
    error = "";
    selectedResultIndex = -1;
    try {
      searchResults = await aiSearch(searchQuery, 20);
    } catch (e: any) {
      error = e?.toString() || "Search failed";
    } finally {
      isLoading = false;
    }
  }

  // ─── Ask ───────────────────────────────────────────────────────────────────

  // Default Ask to the user's current block. That's the safest scope in a
  // block outliner and avoids accidental huge prompts from journal feeds.
  type AskScope = "block" | "page" | "graph";
  let askScope = $state<AskScope>("block");

  function shouldUseWebResearchForBlock(question: string): boolean {
    return /\b(check|fact[- ]?check|verify|validate|source|sources|citation|citations|research|web|internet|look up|accurate|accuracy|true|claim|claims)\b/i.test(question);
  }

  function webResearchAnswerText(result: WebResearchResult): string {
    const parts: string[] = [];
    if (result.title_answer) parts.push(result.title_answer);
    for (const topic of result.topics) {
      const tags = topic.tags?.length
        ? `\n\nTags: ${topic.tags.map((tag) => `#${tag.qualified ?? tag.term}`).join(" ")}`
        : "";
      parts.push(`### ${topic.topic}\n\n${topic.summary}${tags}`);
    }
    return parts.join("\n\n");
  }

  function webSourcesFromResearch(result: WebResearchResult): WebSource[] {
    return result.citations.map((citation) => ({
      number: citation.number,
      title: citation.title,
      url: citation.url,
    }));
  }

  function formatAskThreadForPrompt(maxChars = 14000): string {
    if (askTurns.length === 0) return "";
    const text = askTurns
      .slice(-8)
      .map((turn, index) =>
        `Turn ${index + 1} (${turn.scope}${turn.pageTitle ? `, ${turn.pageTitle}` : ""})\n`
        + `Q: ${turn.question}\nA: ${turn.answer}`
        + (turn.webResearchResult?.citations.length
          ? `\nSources: ${turn.webResearchResult.citations.map((c) => `[${c.number}] ${c.title} (${c.url})`).join("; ")}`
          : "")
      )
      .join("\n\n");
    return text.length > maxChars
      ? text.slice(text.length - maxChars) + "\n[…earlier Ask thread truncated…]"
      : text;
  }

  function addAskTurn(turn: Omit<AskTurn, "id">) {
    askTurns = [...askTurns, { ...turn, id: newOpId() }];
  }

  async function scrollAskToBottom() {
    await tick();
    if (askScrollEl) {
      askScrollEl.scrollTop = askScrollEl.scrollHeight;
    }
  }

  function beginAskMessage(question: string, webResearch = false): number {
    const assistantIndex = askMessages.length + 1;
    askMessages = [
      ...askMessages,
      { role: "user", content: question },
      { role: "assistant", content: "", webResearch },
    ];
    askPendingAssistantIndex = assistantIndex;
    askThinkingTone = webResearch ? "web" : "thinking";
    askThinkingLabel = webResearch ? "Starting web research…" : "Thinking…";
    void scrollAskToBottom();
    return assistantIndex;
  }

  function updateAskAssistant(
    assistantIndex: number,
    content: string,
    extras: Partial<ChatMessageModel> = {},
  ) {
    askMessages = askMessages.map((message, index) =>
      index === assistantIndex
        ? { ...message, ...extras, role: "assistant", content }
        : message,
    );
    void scrollAskToBottom();
  }

  function finishAskMessage(
    assistantIndex: number,
    content: string,
    extras: Partial<ChatMessageModel> = {},
  ) {
    updateAskAssistant(assistantIndex, content, extras);
    if (askPendingAssistantIndex === assistantIndex) {
      askPendingAssistantIndex = null;
    }
  }

  function setAskThinking(label: string, tone: ChatThinkingTone = "thinking") {
    askThinkingLabel = label || (tone === "web" ? "Researching web…" : "Thinking…");
    askThinkingTone = tone;
    void scrollAskToBottom();
  }

  function handleAskKeydown(e: KeyboardEvent) {
    if (e.key !== "Enter" || e.shiftKey) return;
    e.preventDefault();
    void doAsk();
  }

  async function doAsk() {
    if (isLoading) return;
    const question = askQuery.trim();
    if (!question) return;
    askThreadGeneration += 1;
    const generation = askThreadGeneration;
    activeAskGeneration = generation;
    const willUseWebResearch = askScope === "block" && shouldUseWebResearchForBlock(question);
    const assistantIndex = beginAskMessage(question, willUseWebResearch);
    isLoading = true;
    error = "";
    askAnswer = "";
    askWebResearchResult = null;
    askWebResearchProgress = "";
    blockDraftStatus = "";
    askQuery = "";
    try {
      if (askScope === "block") {
        if (willUseWebResearch) {
          const { result, context } = await runCurrentBlockWebResearch(question);
          if (generation !== askThreadGeneration) return;
          const answer = webResearchAnswerText(result);
          askAnswer = answer;
          finishAskMessage(assistantIndex, answer, {
            webResearch: true,
            webSources: webSourcesFromResearch(result),
          });
          addAskTurn({
            scope: "block",
            question,
            answer,
            pageTitle: context.pageTitle,
            blockId: context.blockId,
            webResearchResult: result,
          });
          return;
        }
        const blockContext = await buildCurrentBlockContext(question);
        const thread = formatAskThreadForPrompt();
        const scopedQuestion =
          `You are looking at the page titled "${blockContext.pageTitle}". Answer the user's question using ONLY `
          + `the current block context below and the prior Ask thread when it is relevant. If the block and thread `
          + `do not contain enough information, say so plainly.\n\n`
          + `--- CURRENT BLOCK CONTEXT ---\n${blockContext.text}\n--- END CURRENT BLOCK CONTEXT ---\n\n`
          + (thread ? `--- PRIOR ASK THREAD ---\n${thread}\n--- END PRIOR ASK THREAD ---\n\n` : "")
          + `Question: ${question}`;
        const result = await aiAsk(scopedQuestion);
        if (generation !== askThreadGeneration) return;
        const answer = result.answer;
        askAnswer = answer;
        finishAskMessage(assistantIndex, answer, { sources: result.sources });
        addAskTurn({
          scope: "block",
          question,
          answer,
          pageTitle: blockContext.pageTitle,
          blockId: blockContext.blockId,
        });
      } else if (askScope === "page" && pageId) {
        // Page-scoped: fetch the page's blocks, inline them as context in
        // the question itself. Bypasses the semantic search step so the
        // answer is always grounded in exactly this page's content, even
        // when the embedder isn't reachable or hasn't been indexed yet.
        const pageContext = await buildPageContextForAsk();
        const thread = formatAskThreadForPrompt();
        const scopedQuestion = pageContext.text
          ? `You are looking at the page titled "${pageContext.pageTitle}". Answer the user's `
            + `question using ONLY the page content below plus the prior Ask thread when relevant. If the page and thread don't `
            + `contain enough information, say so plainly.\n\n`
            + `--- PAGE CONTENT ---\n${pageContext.text}\n--- END PAGE CONTENT ---\n\n`
            + (thread ? `--- PRIOR ASK THREAD ---\n${thread}\n--- END PRIOR ASK THREAD ---\n\n` : "")
            + `Question: ${question}`
          : question;
        const result = await aiAsk(scopedQuestion);
        if (generation !== askThreadGeneration) return;
        const answer = result.answer;
        askAnswer = answer;
        finishAskMessage(assistantIndex, answer, { sources: result.sources });
        addAskTurn({ scope: "page", question, answer, pageTitle: pageContext.pageTitle });
      } else {
        const thread = formatAskThreadForPrompt();
        const prompt = thread
          ? `Use the prior Ask thread below as conversation context, then answer the follow-up question across my notes.\n\n`
            + `--- PRIOR ASK THREAD ---\n${thread}\n--- END PRIOR ASK THREAD ---\n\nQuestion: ${question}`
          : question;
        const result = await aiAsk(prompt);
        if (generation !== askThreadGeneration) return;
        const answer = result.answer;
        askAnswer = answer;
        finishAskMessage(assistantIndex, answer, { sources: result.sources });
        addAskTurn({ scope: "graph", question, answer });
      }
    } catch (e: any) {
      if (generation !== askThreadGeneration) return;
      const message = e?.toString() || "Ask failed";
      error = message;
      finishAskMessage(assistantIndex, `**Error:** ${message}`);
    } finally {
      if (generation === askThreadGeneration && askPendingAssistantIndex === assistantIndex) {
        askPendingAssistantIndex = null;
      }
      if (activeAskGeneration === generation) {
        activeAskGeneration = null;
        isLoading = false;
      }
    }
  }

  async function runCurrentBlockWebResearch(focusQuestion: string): Promise<{ result: WebResearchResult; context: CurrentBlockContext }> {
    if (isResearchingWeb) {
      throw new Error("Another web research run is already in progress.");
    }
    const blockContext = await buildCurrentBlockContext(focusQuestion);
    const focus = focusQuestion.trim();
    const thread = formatAskThreadForPrompt();
    const opId = newOpId();
    isResearchingWeb = true;
    webResearchOpId = opId;
    webResearchCancelling = false;
    askWebResearchProgress = "Starting block fact-check...";
    setAskThinking(askWebResearchProgress, "web");
    webResearchProgress = askWebResearchProgress;
    const unlisten = await listen<string>("ai-web-research-progress", (e) => {
      askWebResearchProgress = e.payload;
      setAskThinking(askWebResearchProgress, "web");
      webResearchProgress = e.payload;
    });
    try {
      const result = await aiResearchWeb(
        focus ? `Fact-check: ${focus}` : `Fact-check: ${blockContext.pageTitle}`,
        `Current page: ${blockContext.pageTitle}\n\nCurrent block and nested children:\n${blockContext.text}\n\n`
          + (thread ? `Prior Ask thread:\n${thread}\n\n` : "")
          + `Research focus: ${focus || "Fact-check the factual claims in this block. If a claim cannot be verified from reliable web sources, say so plainly."}`,
        opId,
      );
      askWebResearchResult = result;
      return { result, context: blockContext };
    } finally {
      unlisten();
      askWebResearchProgress = "";
      webResearchProgress = "";
      webResearchOpId = null;
      isResearchingWeb = false;
      webResearchCancelling = false;
    }
  }

  async function summarizeAskThread() {
    if (askTurns.length === 0 || isSummarizingAskThread || isLoading) return;
    isSummarizingAskThread = true;
    error = "";
    blockDraftStatus = "";
    try {
      const prompt =
        `Summarize this Ask thread into a concise, useful answer. Keep factual details, unresolved uncertainties, `
        + `and source references like [1] when present. Do not invent anything beyond the thread.\n\n`
        + `--- ASK THREAD ---\n${formatAskThreadForPrompt(18000)}\n--- END ASK THREAD ---`;
      askThreadSummary = (await aiAsk(prompt)).answer;
    } catch (e: any) {
      error = e?.toString() || "Failed to summarize answers";
    } finally {
      isSummarizingAskThread = false;
    }
  }

  function cleanImprovedBlockDraft(raw: string): string {
    let text = raw.trim();
    const fenced = text.match(/^```(?:markdown|md)?\s*\n([\s\S]*?)\n```$/i);
    if (fenced) text = fenced[1].trim();
    return text
      .replace(/^(?:updated|improved|rewritten)\s+block\s*:\s*/i, "")
      .trim();
  }

  async function draftImprovedCurrentBlock() {
    if (askTurns.length === 0 || isImprovingBlock || isLoading) return;
    isImprovingBlock = true;
    error = "";
    improvedBlockDraft = "";
    improvedBlockContext = null;
    blockDraftStatus = "";
    try {
      const context = await buildCurrentBlockContext(askQuery);
      const prompt =
        `Act like a careful code-edit assistant, but for a Markdown knowledge block. Draft the exact replacement `
        + `for the current block root by merging the useful answer(s) from the Ask thread into the original block. `
        + `Preserve the user's intent, voice, links, tags, task markers, dates, and any personal notes unless the Ask thread `
        + `directly corrects them. Keep the result concise and readable. Do not add unsupported claims; when a claim is uncertain, `
        + `word it as uncertain instead of presenting it as fact. If citations are useful, keep citation markers like [1]. `
        + `Return ONLY the replacement Markdown for the current block root, not commentary and not a fenced code block.\n\n`
        + `--- ORIGINAL CURRENT BLOCK ROOT ---\n${context.rootContent}\n--- END ORIGINAL CURRENT BLOCK ROOT ---\n\n`
        + `--- CURRENT BLOCK WITH CHILD CONTEXT ---\n${context.text}\n--- END CURRENT BLOCK WITH CHILD CONTEXT ---\n\n`
        + `--- ASK THREAD ---\n${formatAskThreadForPrompt(18000)}\n--- END ASK THREAD ---`;
      improvedBlockDraft = cleanImprovedBlockDraft((await aiAsk(prompt)).answer);
      improvedBlockContext = context;
    } catch (e: any) {
      error = e?.toString() || "Failed to improve current block";
    } finally {
      isImprovingBlock = false;
    }
  }

  async function replaceCurrentBlockWithDraft() {
    const context = improvedBlockContext;
    const draft = improvedBlockDraft;
    if (!draft.trim() || !context || isApplyingBlockDraft) return;
    isApplyingBlockDraft = true;
    error = "";
    blockDraftStatus = "";
    try {
      const blocks = await listBlocks(context.pageId);
      const current = blocks.find((b) => b.id === context.blockId);
      if (!current) throw new Error("The current block no longer exists. Click the block again and retry.");
      if (current.content === draft) {
        blockDraftStatus = "Current block already matches the draft.";
        return;
      }
      await updateBlock(context.blockId, draft);
      pushUndo({
        type: "update_block",
        pageId: context.pageId,
        blockId: context.blockId,
        beforeContent: current.content,
        afterContent: draft,
      });
      window.dispatchEvent(new CustomEvent("page-content-reload-blocks", { detail: { pageId: context.pageId } }));
      blockDraftStatus = "Updated current block.";
    } catch (e: any) {
      error = e?.toString() || "Failed to replace current block";
    } finally {
      isApplyingBlockDraft = false;
    }
  }

  function clearAskThread() {
    askThreadGeneration += 1;
    askTurns = [];
    askMessages = [];
    askPendingAssistantIndex = null;
    askAnswer = "";
    askWebResearchResult = null;
    askThreadSummary = "";
    improvedBlockDraft = "";
    improvedBlockContext = null;
    blockDraftStatus = "";
    error = "";
  }

  // Concatenate all blocks on the current page into a single plain-text
  // prompt context. Preserves outline order (list_blocks is already
  // depth-first ordered) so the LLM sees the page the way the reader
  // sees it. Returns an empty string if there are no blocks / the fetch
  // failed, so the caller can fall back to a graph-wide ask.
  type AskPageContext = {
    pageId: string;
    pageTitle: string;
    text: string;
  };

  async function resolvePageContextTarget(): Promise<{ pageId: string; pageTitle: string }> {
    const focusedAnchor = preferFocusedPageForPageScope
      ? askBlockAnchor ?? getLatestCurrentBlockAnchor()
      : null;
    const contextPageId = focusedAnchor?.pageId ?? pageId;
    if (!contextPageId) return { pageId: "", pageTitle };
    if (contextPageId === pageId) return { pageId: contextPageId, pageTitle };
    const focusedPage = await getPage({ id: contextPageId }).catch(() => null);
    return { pageId: contextPageId, pageTitle: focusedPage?.title ?? pageTitle };
  }

  async function buildPageContextForAsk(): Promise<AskPageContext> {
    const target = await resolvePageContextTarget();
    return {
      ...target,
      text: target.pageId ? await buildPageContext(target.pageId) : "",
    };
  }

  async function buildPageContext(contextPageId = pageId): Promise<string> {
    try {
      const blocks = await listBlocks(contextPageId);
      const lines: string[] = [];
      for (const b of blocks) {
        const text = (b.content ?? "").trim();
        if (text) lines.push(text);
      }
      // Guardrail: keep context under ~24k chars so we don't accidentally
      // blow the model's context window on very large pages.
      const joined = lines.join("\n");
      const MAX = 24000;
      return joined.length > MAX
        ? joined.slice(0, MAX) + "\n[…page truncated for prompt length…]"
        : joined;
    } catch {
      return "";
    }
  }

  type CurrentBlockContext = {
    pageId: string;
    pageTitle: string;
    blockId: string;
    rootContent: string;
    text: string;
  };

  function visibleBlockAnchorFromQuestion(question: string): CurrentBlockAnchor | null {
    if (typeof document === "undefined") return null;
    const terms = question
      .toLowerCase()
      .match(/[a-z0-9][a-z0-9_-]{2,}/g)
      ?.filter((term) => !["about", "block", "check", "these", "this", "that", "please", "with", "from", "what", "when", "where"].includes(term))
      ?? [];
    if (terms.length === 0) return null;

    let best: { score: number; anchor: CurrentBlockAnchor } | null = null;
    for (const el of Array.from(document.querySelectorAll<HTMLElement>(".block-item[data-block-id][data-page-id]"))) {
      const text = (el.textContent ?? "").toLowerCase();
      let score = 0;
      for (const term of terms) {
        if (text.includes(term)) score += term.length;
      }
      if (score > (best?.score ?? 0)) {
        best = {
          score,
          anchor: {
            pageId: el.dataset.pageId ?? pageId,
            blockId: el.dataset.blockId ?? null,
          },
        };
      }
    }
    return best?.score ? best.anchor : null;
  }

  function askQuestionTerms(question: string): string[] {
    return question
      .toLowerCase()
      .match(/[a-z0-9][a-z0-9_-]{2,}/g)
      ?.filter((term) => !["about", "block", "check", "these", "this", "that", "please", "with", "from", "what", "when", "where", "better", "give", "tell"].includes(term))
      ?? [];
  }

  function scoreTextAgainstTerms(text: string, terms: string[]): number {
    const lower = text.toLowerCase();
    return terms.reduce((score, term) => lower.includes(term) ? score + term.length : score, 0);
  }

  async function searchedBlockAnchorFromQuestion(question: string): Promise<CurrentBlockAnchor | null> {
    const terms = askQuestionTerms(question);
    if (terms.length === 0) return null;
    try {
      const results = await searchFts(terms.slice(0, 6).join(" "), 20);
      let best: { score: number; block: Block } | null = null;
      for (const block of results) {
        const score = scoreTextAgainstTerms(block.content ?? "", terms);
        if (score > (best?.score ?? 0)) {
          best = { score, block };
        }
      }
      return best?.score
        ? { pageId: best.block.page_id, blockId: best.block.id }
        : null;
    } catch {
      return null;
    }
  }

  function resolveAskBlockAnchor(question: string): CurrentBlockAnchor | null {
    return askBlockAnchor
      ?? getLatestCurrentBlockAnchor()
      ?? visibleBlockAnchorFromQuestion(question);
  }

  async function buildCurrentBlockContext(question = ""): Promise<CurrentBlockContext> {
    let anchor = resolveAskBlockAnchor(question);
    if (!anchor?.blockId) {
      anchor = await searchedBlockAnchorFromQuestion(question);
    }
    if (!anchor?.pageId) {
      throw new Error("Open a page first to use current-block Ask.");
    }
    if (!anchor.blockId) {
      throw new Error("Click into a block first, then use Current block.");
    }

    askBlockAnchor = anchor;
    if (anchor.pageId === pageId) {
      lastFocusedBlockId = anchor.blockId;
    }

    const blocks = await listBlocks(anchor.pageId);
    const root = blocks.find((b) => b.id === anchor.blockId);
    if (!root) {
      throw new Error("The last focused block is no longer on this page. Click the block again and retry.");
    }
    const contextPageTitle = anchor.pageId === pageId
      ? pageTitle
      : (await getPage({ id: anchor.pageId }).catch(() => null))?.title ?? "Current block page";

    const childrenByParent = new Map<string, Block[]>();
    for (const block of blocks) {
      if (!block.parent_id) continue;
      const siblings = childrenByParent.get(block.parent_id) ?? [];
      siblings.push(block);
      childrenByParent.set(block.parent_id, siblings);
    }
    for (const siblings of childrenByParent.values()) {
      siblings.sort((a, b) => a.order_index - b.order_index);
    }

    const lines: string[] = [];
    const visit = (block: Block, depth: number) => {
      const text = (block.content ?? "").trim();
      if (text) lines.push(`${"  ".repeat(depth)}- ${text}`);
      for (const child of childrenByParent.get(block.id) ?? []) {
        visit(child, depth + 1);
      }
    };
    visit(root, 0);

    if (lines.length === 0) {
      throw new Error("The current block is empty.");
    }
    const joined = lines.join("\n");
    const MAX = 12000;
    const text = joined.length > MAX
      ? joined.slice(0, MAX) + "\n[…block context truncated for prompt length…]"
      : joined;
    return {
      pageId: anchor.pageId,
      pageTitle: contextPageTitle,
      blockId: anchor.blockId,
      rootContent: root.content ?? "",
      text,
    };
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
  <aside class="reference-panel" class:panel-visible={visible} style="width: {width}px;">
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
      {#if activeTab === "search"}
        <!-- Search doesn't require AI at all -- "Quick matches" below is
             plain SQLite FTS (build_fts_prefix_query in core/src/db/blocks.rs),
             so it works with no model configured/reachable at all (e.g. on a
             phone build with no local LLM). Only the "AI Search" button
             (semantic search) needs a working embedder, and it already
             degrades gracefully via its own try/catch in doSearch() --
             so unlike References/Ask below, this tab is never gated behind
             the health check. -->
        <div class="tab-content">
          <form class="search-form" onsubmit={(e) => { e.preventDefault(); doSearch(); }}>
            <input
              type="text"
              bind:this={searchInputEl}
              bind:value={searchQuery}
              oninput={handleSearchInput}
              onkeydown={handleSearchKeydown}
              placeholder="Type to find pages & blocks (e.g. 'magn' finds 'magnesium')..."
              class="search-input"
            />
            <button
              type="button"
              class="action-btn"
              onclick={doPlainSearch}
              disabled={!searchQuery.trim()}
              title="Plain text search (substring match on page titles & block content) -- no AI/model involved at all"
            >
              Search
            </button>
            <button
              type="submit"
              class="action-btn"
              disabled={isLoading || !health?.embedder_available}
              title={health?.embedder_available
                ? "Deeper AI semantic search across all graphs"
                : "AI semantic search needs a configured, reachable embedding model -- text search above still works without it"}
            >
              {isLoading ? "..." : "AI Search"}
            </button>
          </form>

          {#if !health?.embedder_available}
            <div class="panel-notice notice-sub-only">
              AI semantic search isn't available (no embedding model configured/reachable) --
              text search above still works.
            </div>
          {/if}

          {#if error}
            <div class="error-msg">{error}</div>
          {/if}

          {#if quickMatches.length > 0}
            <div class="quick-matches-label">
              Quick matches{isQuickSearching ? "…" : ""}
            </div>
            {#each quickMatches as match, i}
              {#if match.kind === "page"}
                <button
                  class="search-result quick-match"
                  class:selected={selectedResultIndex === i}
                  onclick={() => navigateToQuickMatch(match)}
                >
                  <div class="result-header">
                    <span class="result-kind-tag">Page</span>
                    <span class="result-title">{match.page.title}</span>
                  </div>
                </button>
              {:else}
                <button
                  class="search-result quick-match"
                  class:selected={selectedResultIndex === i}
                  onclick={() => navigateToQuickMatch(match)}
                >
                  <div class="result-header">
                    <span class="result-kind-tag">Block</span>
                    <span class="result-title">{match.pageTitle}</span>
                  </div>
                  <div class="result-snippet">{match.block.content.replace(/^[-*>\s#]+/, "").slice(0, 160) || "(empty block)"}</div>
                </button>
              {/if}
            {/each}
          {/if}

          {#if searchResults.length > 0}
            <div class="quick-matches-label">AI semantic results</div>
          {/if}
          {#each searchResults as result, j}
            <button
              class="search-result"
              class:selected={selectedResultIndex === quickMatches.length + j}
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
      {:else if !health?.enabled}
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
                onclick={findLinksForCurrentPage}
                disabled={!pageId}
                title={pageId ? `Find reviewable link suggestions for ${pageTitle || "this page"}` : "Open a page to find links"}
              >
                Find links
              </button>
              <button
                class="action-btn"
                onclick={generateReferences}
                disabled={isLoading || !pageId}
              >
                {isLoading ? "Summarizing..." : "Summarize this Page"}
              </button>
              <button
                class="action-btn"
                onmousedown={captureSelectionOnMouseDown}
                onclick={analyzeSelection}
                disabled={isAnalyzingSelection || !hasTextSelection || !pageId}
                title={hasTextSelection ? "Summarize the highlighted text" : "Highlight some text on the page first"}
              >
                {isAnalyzingSelection ? "Summarizing selection..." : "Summarize Selection"}
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
                <button
                  class="cancel-btn"
                  onclick={() => cancelOperation("analyze-page")}
                  disabled={analyzePageCancelling}
                  title="Stop this analysis. The local model will be restarted, so the next run may take a few extra seconds to start."
                >
                  {analyzePageCancelling ? "Cancelling…" : "Cancel"}
                </button>
              </div>
            {/if}

            {#if selectionError}
              <div class="error-msg">{selectionError}</div>
            {/if}

            {#if isAnalyzingSelection && selectionProgress}
              <div class="progress-status">
                <span class="progress-spinner"></span>
                <span class="progress-text">{selectionProgress}</span>
                <button
                  class="cancel-btn"
                  onclick={() => cancelOperation("analyze-selection")}
                  disabled={analyzeSelectionCancelling}
                  title="Stop this analysis. The local model will be restarted, so the next run may take a few extra seconds to start."
                >
                  {analyzeSelectionCancelling ? "Cancelling…" : "Cancel"}
                </button>
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
                <button
                  class="cancel-btn"
                  onclick={() => cancelOperation("web-research")}
                  disabled={webResearchCancelling}
                  title="Stop this research. The local model will be restarted, so the next run may take a few extra seconds to start."
                >
                  {webResearchCancelling ? "Cancelling…" : "Cancel"}
                </button>
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
            {:else if references?.summary_error}
              <!--
                Summary generation was *attempted and failed* (bad JSON,
                broken reasoning-tag training, provider hiccup, ...).
                Show the reason inline so the user can act on it
                (usually: pick a saner chat model in Settings) instead
                of just staring at an empty panel.
              -->
              <div class="summary-error-card" role="alert">
                <div class="summary-error-title">Couldn't produce a summary</div>
                <div class="summary-error-text">{references.summary_error}</div>
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
                Click "Summarize this Page" to generate a summary and discover connections.
              </div>
            {/if}
          </div>

        <!-- Ask Tab -->
        {:else if activeTab === "ask"}
          <div class="tab-content ask-tab">
            <div class="ask-scroll" bind:this={askScrollEl}>
              {#if askTurns.length > 0}
                <div class="ask-thread-actions">
                  <button
                    type="button"
                    class="scope-btn"
                    onclick={summarizeAskThread}
                    disabled={isLoading || isSummarizingAskThread}
                  >
                    {isSummarizingAskThread ? "Summarizing..." : "Summarize answers"}
                  </button>
                  {#if askScope === "block"}
                    <button
                      type="button"
                      class="scope-btn"
                      onclick={draftImprovedCurrentBlock}
                      disabled={isLoading || isImprovingBlock}
                      title="Draft a replacement by merging the AI answer with the current block"
                    >
                      {isImprovingBlock ? "Merging..." : "Merge answer with block"}
                    </button>
                  {/if}
                  <button type="button" class="scope-btn" onclick={clearAskThread} disabled={isLoading}>
                    Clear thread
                  </button>
                </div>
              {/if}

              {#if error}
                <div class="error-msg">{error}</div>
              {/if}

              {#if isLoading && askWebResearchProgress}
                <div class="progress-status">
                  <span class="progress-spinner"></span>
                  <span class="progress-text">{askWebResearchProgress}</span>
                  <button
                    class="cancel-btn"
                    onclick={() => cancelOperation("web-research")}
                    disabled={webResearchCancelling}
                    title="Stop this web fact-check. The local model will be restarted, so the next run may take a few extra seconds to start."
                  >
                    {webResearchCancelling ? "Cancelling…" : "Cancel"}
                  </button>
                </div>
              {/if}

              {#if askThreadSummary}
                <div class="summary-card">
                  <div class="summary-card-label">Answer summary</div>
                  <div class="summary-text">{askThreadSummary}</div>
                </div>
              {/if}

              {#if improvedBlockDraft}
                <div class="summary-card">
                  <div class="summary-card-label">Merged block draft</div>
                  <pre class="block-draft">{improvedBlockDraft}</pre>
                  <button
                    type="button"
                    class="insert-summary-btn"
                    onclick={replaceCurrentBlockWithDraft}
                    disabled={isApplyingBlockDraft}
                  >
                    {isApplyingBlockDraft ? "Replacing..." : "Replace current block"}
                  </button>
                  {#if blockDraftStatus}
                    <div class="scope-hint">{blockDraftStatus}</div>
                  {/if}
                </div>
              {/if}

              {#if askMessages.length > 0}
                <div class="ask-thread">
                  {#each askMessages as message, index}
                    <ChatMessageBubble
                      {message}
                      {index}
                      streaming={askPendingAssistantIndex === index}
                      animateCursor={askPendingAssistantIndex === index}
                      thinkingLabel={askPendingAssistantIndex === index ? askThinkingLabel : ""}
                      thinkingTone={askThinkingTone}
                      onOpenWebSource={openChatWebSource}
                    />
                  {/each}
                </div>
              {:else if !error && !isLoading}
                <div class="panel-notice ask-empty">
                  Ask a question, then keep asking follow-ups. The input stays pinned here.
                </div>
              {/if}
            </div>

            <div class="ask-composer">
              <div class="ask-scope">
                <span class="ask-scope-label">Scope:</span>
                <button
                  type="button"
                  class="scope-btn"
                  class:active={askScope === "block"}
                  onclick={() => (askScope = "block")}
                  disabled={!pageId}
                  title={lastFocusedBlockId ? "Ask about the block you last clicked or edited" : "Click into a block first to enable current-block Ask"}
                >
                  Current block
                </button>
                <button
                  type="button"
                  class="scope-btn"
                  class:active={askScope === "page"}
                  onclick={() => (askScope = "page")}
                  disabled={!pageId}
                  title={pageId ? "Ask about the current page only" : "Open a page first to enable page-scoped Ask"}
                >
                  This page
                </button>
                <button
                  type="button"
                  class="scope-btn"
                  class:active={askScope === "graph"}
                  onclick={() => (askScope = "graph")}
                  title="Ask across all pages using semantic search"
                >
                  All notes
                </button>
              </div>
              {#if askScope === "block"}
                <div class="scope-hint" class:warning={!askBlockAnchor?.blockId}>
                  {askBlockAnchor?.blockId
                    ? "Using the block you last clicked or edited, including nested children."
                    : "Click a block first, or mention the visible block title in your question."}
                </div>
              {:else if askScope === "page" && preferFocusedPageForPageScope}
                <div class="scope-hint">
                  Using the journal day where your cursor/current block is, not the whole journal feed.
                </div>
              {/if}
              <form class="search-form ask-form" onsubmit={(e) => { e.preventDefault(); doAsk(); }}>
                <textarea
                  bind:value={askQuery}
                  placeholder={askScope === "block"
                    ? "Ask about or fact-check the current block..."
                    : askScope === "page"
                      ? "Ask a question about this page..."
                      : "Ask a question about your knowledge..."}
                  class="search-input ask-input"
                  rows="2"
                  onkeydown={handleAskKeydown}
                ></textarea>
                <button
                  type="submit"
                  class="action-btn"
                  disabled={isLoading || (askScope === "block" && !pageId)}
                >
                  {isLoading ? "Thinking..." : "Ask"}
                </button>
              </form>
            </div>
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
    max-width: calc(100vw - 24px);
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
    min-height: 0;
    overflow: hidden;
    padding: 12px;
    display: flex;
    flex-direction: column;
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

  .panel-notice.notice-sub-only {
    text-align: left;
    padding: 4px 2px;
    font-size: 12px;
    opacity: 0.7;
  }

  .tab-content {
    display: flex;
    flex-direction: column;
    gap: 12px;
    flex: 1;
    min-height: 0;
  }

  .tab-content:not(.ask-tab) {
    overflow-y: auto;
  }

  .ask-tab {
    gap: 0;
  }

  .ask-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding-bottom: 12px;
  }

  .ask-composer {
    flex-shrink: 0;
    margin: 0 -12px -12px;
    padding: 10px 12px 12px;
    background: var(--bg-secondary, #1e1e2e);
    border-top: 1px solid var(--border-color, #333);
    box-shadow: 0 -8px 20px rgba(0, 0, 0, 0.16);
  }

  .tab-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
  }

  .action-btn {
    background: var(--btn-primary-bg, var(--accent-color, #7c3aed));
    color: var(--btn-primary-fg, var(--bg-primary));
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

  .action-btn.secondary {
    background: var(--bg-tertiary, #252535);
    color: var(--text-primary, #fff);
    border: 1px solid var(--border-color, #333);
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
    flex: 1;
  }

  /* Cancel button that shows up next to a running AI operation's
     progress-text. Kept small and unobtrusive so the streamed model
     output stays the primary focus, but clearly clickable — the whole
     point is that the user can bail out any time. Sticks to the top of
     the row rather than centering so a growing progress-text (as more
     tokens stream in) doesn't jitter the button around. */
  .cancel-btn {
    align-self: flex-start;
    background: transparent;
    color: var(--text-secondary, #aaa);
    border: 1px solid var(--border-color, #444);
    border-radius: 4px;
    padding: 3px 10px;
    font-size: 11px;
    cursor: pointer;
    white-space: nowrap;
    flex-shrink: 0;
    margin-top: 1px;
  }

  .cancel-btn:hover:not(:disabled) {
    background: rgba(220, 38, 38, 0.15);
    border-color: rgba(220, 38, 38, 0.4);
    color: #f87171;
  }

  .cancel-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
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

  .summary-error-card {
    background: var(--bg-tertiary, #252535);
    border: 1px solid #b45309;
    border-left: 3px solid #f59e0b;
    border-radius: 8px;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .summary-error-title {
    color: #f59e0b;
    font-size: 12px;
    font-weight: 600;
  }
  .summary-error-text {
    color: var(--text-secondary, #b8b8c8);
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
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
    color: var(--btn-primary-fg, var(--bg-primary));
    background: var(--btn-primary-bg, var(--accent-color, #7c3aed));
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
    flex-wrap: wrap;
  }

  .ask-form {
    align-items: flex-end;
    flex-wrap: nowrap;
  }

  .ask-scope {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 8px;
    font-size: 12px;
  }

  .ask-scope-label {
    color: var(--text-muted, #888);
    margin-right: 4px;
  }

  .scope-hint {
    color: var(--text-muted, #888);
    font-size: 11px;
    line-height: 1.4;
    margin: -2px 0 8px;
  }

  .scope-hint.warning {
    color: var(--warning-color, #f59e0b);
  }

  .scope-btn {
    background: var(--bg-tertiary, #252535);
    border: 1px solid var(--border-color, #333);
    color: var(--text-secondary, #ccc);
    padding: 4px 10px;
    border-radius: 999px;
    font-size: 12px;
    cursor: pointer;
    transition: border-color 0.15s, background 0.15s;
  }

  .scope-btn:hover:not(:disabled) {
    border-color: var(--accent-color, #7c3aed);
  }

  .scope-btn.active {
    background: var(--btn-primary-bg, var(--accent-color, #7c3aed));
    color: var(--btn-primary-fg, var(--bg-primary));
    border-color: var(--btn-primary-bg, var(--accent-color, #7c3aed));
  }

  .scope-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
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

  .ask-input {
    min-height: 44px;
    max-height: 160px;
    resize: vertical;
    font-family: inherit;
    line-height: 1.4;
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

  .search-result.selected {
    border-color: var(--accent-color, #7c3aed);
    background: var(--bg-hover, #2d2d40);
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

  .quick-matches-label {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.03em;
    color: var(--text-muted, #888);
    margin: 4px 0 -2px;
  }

  .result-kind-tag {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    color: var(--accent-color, #7c3aed);
    background: var(--bg-secondary, #1a1a24);
    border-radius: 4px;
    padding: 1px 5px;
    margin-right: 6px;
  }

  .quick-match .result-header {
    justify-content: flex-start;
  }

  .ask-thread-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .ask-thread {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .block-draft {
    margin: 0;
    padding: 10px;
    background: var(--bg-secondary, #1a1a24);
    border: 1px solid var(--border-color, #333);
    border-radius: 6px;
    color: var(--text-primary, #fff);
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace;
    font-size: 12px;
    line-height: 1.5;
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
