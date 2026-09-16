<script lang="ts">
  import { listen } from "@tauri-apps/api/event";
  import AIWritingPanel from "./AIWritingPanel.svelte";
  import { readingSourceBlocks } from "../lib/readingNoteFormat";
  import { readingSelection } from "../lib/readingSelection";
  import {
    aiGenerateReferences, aiHealthCheck, aiAsk, aiInsertPageSummary, aiSummarizeSelection, aiCancelOperation,
    type PageReferencesMeta, type PageSummary,
  } from "../lib/knowledge";
  import { captureResearchSource, assertResearchSourceGraph, type ResearchSource } from "../lib/researchSource";
  import { pushUndo, runUndoOperation, notifyWritingChanged } from "../lib/undoStack";
  import { flushPageEditors, withPageEditorsLocked } from "../lib/editorPersistence";
  import { applyWritingChanges } from "../lib/writing";
  import { formatConceptTag } from "../lib/conceptLinks";
  import {
    assistantConversationChanges, assistantConversationRunning,
    type AssistantThread,
  } from "../lib/assistantConversations";
  import type { PageNavigationTarget } from "../lib/navigation";

  let { pageId, pageTitle, blockId = null, thread, active = true, openTools = false,
    onFindLinks, onNavigate = () => {}, onOpenSettings = () => {},
  }: {
    pageId: string; pageTitle: string; blockId?: string | null; thread: AssistantThread;
    active?: boolean; openTools?: boolean;
    onFindLinks?: (page: { id: string; title: string }, exactOnly?: boolean) => void;
    onNavigate?: (target: PageNavigationTarget) => void; onOpenSettings?: () => void;
  } = $props();

  let ready = $state(false);
  let error = $state("");
  let notice = $state("");
  let progress = $state("");
  let operation = $state<{ id: string; kind: "page" | "selection" | "answers" | "merge" } | null>(null);
  let cancelling = $state(false);
  let applying = $state(false);
  let summary = $state<PageSummary | null>(null);
  let summarySource = $state.raw<ResearchSource | null>(null);
  let summaryKind = $state("");
  let references = $state<PageReferencesMeta | null>(null);
  let inserted = $state(false);
  let mergedDraft = $state("");
  let mergeSource = $state.raw<ResearchSource | null>(null);
  let mergeBlockId = $state<string | null>(null);
  let mergeOriginal = $state("");
  let toolsEl = $state<HTMLDetailsElement>();
  let writingEl = $state<HTMLDetailsElement>();
  const running = $derived.by(() => { $assistantConversationChanges; return assistantConversationRunning(thread); });
  const hasAnswers = $derived.by(() => {
    $assistantConversationChanges;
    return thread.messages.some((message) => message.role === "assistant" && message.content.trim());
  });
  const selected = $derived($readingSelection.selection?.pageId === pageId && !$readingSelection.error
    ? $readingSelection.selection : null);
  const unavailable = $derived(!!operation || applying || running);
  const anchor = $derived(blockId ? { pageId, blockId } : null);

  $effect(() => {
    if (!active) return;
    let disposed = false;
    const refresh = () => { void aiHealthCheck().then((health) => { if (!disposed) ready = health.enabled && health.llm_available; }).catch(() => { if (!disposed) ready = false; }); };
    refresh();
    window.addEventListener("ai-configuration-changed", refresh);
    return () => { disposed = true; window.removeEventListener("ai-configuration-changed", refresh); };
  });
  $effect(() => {
    if (!openTools) return;
    if (toolsEl) toolsEl.open = true;
    if (writingEl) writingEl.open = true;
  });

  function sourceIsVisible(source: ResearchSource | null) {
    return !!source && source.pageId === pageId && source.graphPath === thread.graphPath;
  }
  function answerText() {
    return thread.messages.filter((message) => message.content.trim()).slice(-16).map((message) =>
      `${message.role === "user" ? "Question" : "Answer"}: ${message.content}`
      + (message.webSources?.length ? `\nSources: ${message.webSources.map((source) => `[${source.number}] ${source.title} (${source.url})`).join("; ")}` : "")
    ).join("\n\n");
  }
  async function findLinks(exactOnly = false) {
    try {
      const source = await captureResearchSource(pageId, blockId);
      if (onFindLinks) onFindLinks({ id: source.pageId, title: source.pageTitle }, exactOnly || !ready);
      else window.dispatchEvent(new CustomEvent("assistant-find-links", { detail: { page: { id: source.pageId, title: source.pageTitle }, exactOnly: exactOnly || !ready } }));
    } catch (cause) { error = `Could not read the source for linking: ${String(cause)}`; }
  }
  async function summarize(kind: "page" | "selection" | "answers") {
    if (unavailable || !ready || (kind === "selection" && !selected) || (kind === "answers" && !hasAnswers)) return;
    const sourceId = pageId;
    const afterBlockId = blockId;
    const passage = kind === "selection" ? selected?.text : kind === "answers" ? answerText() : null;
    const id = crypto.randomUUID();
    operation = { id, kind }; cancelling = false; error = ""; notice = ""; progress = "Preparing summary…";
    let unlisten: (() => void) | undefined;
    try {
      const source = await captureResearchSource(sourceId, afterBlockId);
      if (cancelling) return;
      unlisten = await listen<string>(kind === "page" ? "ai-reference-progress" : "ai-selection-summary-progress",
        ({ payload }) => { if (operation?.id === id) progress = payload.length > 400 ? `…${payload.slice(-400)}` : payload; });
      if (kind === "page") {
        const result = await aiGenerateReferences(source.pageId, id, { graphPath: source.graphPath, expectedBlocks: source.snapshot });
        if (cancelling) return;
        references = result;
        summary = result.summary ?? null;
        if (!summary && result.summary_error) error = result.summary_error;
      } else {
        const result = await aiSummarizeSelection(passage!, source.pageTitle, id);
        if (cancelling) return;
        summary = result;
        references = null;
      }
      summarySource = source;
      summaryKind = kind === "selection" ? "Selection summary" : kind === "answers" ? "Answer summary" : "Page summary";
      inserted = false;
    } catch (cause) { if (!cancelling) error = String(cause); }
    finally { unlisten?.(); operation = null; cancelling = false; progress = ""; }
  }
  async function cancel() {
    if (!operation || cancelling) return;
    cancelling = true;
    try { await aiCancelOperation(operation.id); notice = "Stopped. No notes changed."; }
    catch (cause) { error = `Could not stop: ${String(cause)}`; cancelling = false; }
  }
  async function writeSummaryIntoPage() {
    const resultSummary = summary;
    const source = summarySource;
    if (!resultSummary || !source || applying || inserted) return;
    applying = true; error = ""; notice = "";
    try {
      if (!sourceIsVisible(source)) throw new Error("Return to the source page before inserting this result.");
      await runUndoOperation(() => withPageEditorsLocked(source.pageId, async () => {
        await flushPageEditors(source.pageId);
        await assertResearchSourceGraph(source);
        if (!sourceIsVisible(source)) throw new Error("The source page changed. Nothing was inserted.");
        const result = await aiInsertPageSummary(source.pageId, resultSummary.title_answer, resultSummary.topics ?? [],
          source.afterBlockId, { graphPath: source.graphPath, expectedBlocks: source.snapshot });
        pushUndo({ type: "insert_summary", ...result });
        inserted = true;
        notice = result.unlinkedTargets.length
          ? `Inserted into ${source.pageTitle}. Left ambiguous concepts unlinked: ${result.unlinkedTargets.map((target) => `${target.targetTitle} (${target.reason})`).join("; ")}`
          : `Inserted into ${source.pageTitle}${result.createdTargets.length ? `; created ${result.createdTargets.length} concept pages` : ""}. Ctrl+Z undoes the insertion.`;
        window.dispatchEvent(new CustomEvent("page-content-reload-blocks", { detail: { pageId: source.pageId } }));
      }));
    } catch (cause) { error = String(cause); }
    finally { applying = false; }
  }
  async function draftMergedBlock() {
    if (unavailable || !ready || !blockId || !hasAnswers) return;
    const targetBlockId = blockId;
    const sourceId = pageId;
    const answers = answerText();
    operation = { id: crypto.randomUUID(), kind: "merge" }; error = ""; notice = ""; progress = "Drafting merged block…";
    try {
      const source = await captureResearchSource(sourceId, targetBlockId);
      const root = readingSourceBlocks(source.snapshot).find((block) => block.id === targetBlockId);
      if (!root) throw new Error("Choose a source block, not an annotation, before merging an answer.");
      const prompt = "Draft the exact replacement Markdown for this block, merging useful answers below. "
        + "Preserve the user's intent, voice, links, tags, task markers, dates, numbers, citations, and personal notes. "
        + "Do not add unsupported claims. Return only replacement Markdown, without commentary or a code fence.\n\n"
        + `ORIGINAL BLOCK:\n${root.content}\n\nCONVERSATION:\n${answers}`;
      const response = await aiAsk(prompt, undefined, [], { pageId: source.pageId, blockId: targetBlockId });
      mergedDraft = response.answer.trim().replace(/^```(?:markdown|md)?\s*\n/i, "").replace(/\n```\s*$/, "").trim();
      mergeSource = source;
      mergeBlockId = targetBlockId;
      mergeOriginal = root.content;
    } catch (cause) { error = String(cause); }
    finally { operation = null; progress = ""; }
  }
  async function applyMergedBlock() {
    const source = mergeSource;
    const id = mergeBlockId;
    const replacement = mergedDraft;
    if (!source || !id || !replacement.trim() || applying) return;
    applying = true; error = ""; notice = "";
    try {
      if (!sourceIsVisible(source)) throw new Error("Return to the source page before applying this draft.");
      await runUndoOperation(() => withPageEditorsLocked(source.pageId, async () => {
        await flushPageEditors(source.pageId);
        await assertResearchSourceGraph(source);
        if (!sourceIsVisible(source)) throw new Error("The source page changed. Nothing was replaced.");
        const changes = [{ blockId: id, beforeContent: mergeOriginal, afterContent: replacement }];
        await applyWritingChanges(source.graphPath, source.pageId, changes, source.snapshot);
        const action = { type: "rewrite_writing" as const, graphPath: source.graphPath, pageId: source.pageId, changes };
        pushUndo(action);
        await notifyWritingChanged(action);
        mergedDraft = ""; mergeSource = null;
        notice = "Updated the captured source block. Ctrl+Z undoes the replacement.";
      }));
    } catch (cause) { error = String(cause); }
    finally { applying = false; }
  }
</script>

<details class="page-tools assistant-disclosure" bind:this={toolsEl}>
  <summary>Page / selection tools</summary>
  <div class="tools-content assistant-disclosure-body">
    <p class="tool-source">{pageTitle || "Current source"} · Tools never insert or replace an answer automatically.</p>
    <div class="tool-actions">
      <button onclick={() => findLinks()} disabled={!pageId} title="Find links across this source page, not only the selected passage">Find links in page</button>
      <button onclick={() => findLinks(true)} disabled={!pageId} title="Exact text matches across this source page; no AI required">Exact page links</button>
      <button onclick={() => summarize("page")} disabled={!ready || unavailable}>Summarize this Page</button>
      <button onmousedown={(event) => event.preventDefault()} onclick={() => summarize("selection")} disabled={!ready || unavailable || !selected}>Summarize Selection</button>
      <button onclick={() => summarize("answers")} disabled={!ready || unavailable || !hasAnswers}>Summarize answers</button>
      <button onclick={draftMergedBlock} disabled={!ready || unavailable || !hasAnswers || !blockId}>Merge answer with block</button>
    </div>
    {#if !ready}<p>Exact link discovery works offline without AI. Connect a model for summaries and writing assistance.</p>{/if}
    {#if operation}
      <p role="status">{cancelling ? "Stopping…" : progress}</p>
      {#if operation.kind !== "merge"}<button onclick={cancel} disabled={cancelling}>Stop summary</button>{/if}
    {/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if notice}<p role="status">{notice}</p>{/if}
    {#if summary && summarySource}
      <section class="tool-result" aria-label={summaryKind}>
        <h4>{summaryKind} · {summarySource.pageTitle}</h4>
        <p>{summary.title_answer}</p>
        {#each summary.topics as topic}
          <h5>{topic.topic}</h5><p>{topic.summary}</p>
          {#if topic.tags?.length}<div class="summary-tags">{#each topic.tags as tag}<span>{formatConceptTag(tag.qualified ?? tag.term)}</span>{/each}</div>{/if}
        {/each}
        <p>Inserts after the captured reading position without changing the source text. One Ctrl+Z undo includes the complete insertion receipt.</p>
        <button onclick={writeSummaryIntoPage} disabled={applying || inserted || !sourceIsVisible(summarySource)}>
          {inserted ? "Inserted into page ✓" : applying ? "Inserting…" : "Insert into page"}
        </button>
      </section>
    {/if}
    {#if references}
      <p>{references.reference_count} references found</p>
      {#each references.references as ref}
        <div class="reference">
          <p>[{ref.ref_number}] “{ref.anchor_text}” · Confidence {Math.round(ref.confidence * 100)}%</p>
          {#each ref.related_pages as related}
            <button class="related-page" onclick={() => onNavigate({ id: related.page_id })}>
              {related.page_title} · {Math.round(related.score * 100)}%
              <span>{related.snippet}</span>
            </button>
          {/each}
        </div>
      {/each}
    {/if}
    {#if mergedDraft && mergeSource}
      <section class="tool-result" aria-label="Merged block draft">
        <h4>Merged block draft · {mergeSource.pageTitle}</h4>
        <p>Review and edit before applying. Concurrent source edits will block replacement.</p>
        <label>Replacement Markdown<textarea bind:value={mergedDraft} disabled={applying} rows="8"></textarea></label>
        <button onclick={applyMergedBlock} disabled={applying || !sourceIsVisible(mergeSource) || !mergedDraft.trim()}>{applying ? "Applying…" : "Replace captured block"}</button>
        <button onclick={() => { mergedDraft = ""; mergeSource = null; }} disabled={applying}>Discard draft</button>
      </section>
    {/if}
    <details class="writing-tools assistant-disclosure" bind:this={writingEl}>
      <summary>Writing assistance</summary>
      <div class="assistant-disclosure-body">
        <AIWritingPanel {pageId} {pageTitle} {anchor} {onOpenSettings} />
      </div>
    </details>
  </div>
</details>

<style>
  .page-tools { font-size: 12px; color: var(--text-secondary); }
  .tools-content { display: flex; flex-direction: column; gap: 10px; min-width: 0; }
  p { margin: 0; line-height: 1.5; overflow-wrap: anywhere; }
  .tool-source { color: var(--text-secondary); }
  .tool-actions { display: flex; flex-wrap: wrap; gap: 6px; }
  button { font: inherit; text-align: left; background: var(--bg-primary); color: var(--text-primary); border: 1px solid var(--border); border-radius: 5px; padding: 5px 8px; cursor: pointer; }
  button:hover:not(:disabled) { background: var(--bg-hover); }
  button:disabled { opacity: .55; cursor: default; }
  button:focus-visible, textarea:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .tool-result, .reference { display: grid; gap: 8px; border-top: 1px solid var(--border); padding-top: 12px; }
  h4, h5 { color: var(--text-primary); font-size: 13px; margin: 0; }
  h5 { margin-top: 6px; }
  .summary-tags { display: flex; flex-wrap: wrap; gap: 8px; color: var(--accent); }
  .related-page span { display: block; margin-top: 4px; color: var(--text-secondary); }
  textarea { box-sizing: border-box; display: block; width: 100%; margin: 6px 0; resize: vertical; font: inherit; background: var(--bg-primary); color: var(--text-primary); border: 1px solid var(--border); padding: 8px; border-radius: 5px; }
  .error { color: var(--danger, #c0392b); }
</style>
