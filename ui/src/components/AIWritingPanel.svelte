<script lang="ts">
  import { onDestroy, onMount, untrack } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { aiHealthCheck, type HealthStatus } from "../lib/knowledge";
  import type { CurrentBlockAnchor } from "../lib/currentBlockAnchor";
  import { flushPageEditors, withPageEditorsLocked } from "../lib/editorPersistence";
  import { notifyWritingChanged, pushUndo, runUndoOperation } from "../lib/undoStack";
  import {
    analyzeWriting, applyWritingChanges, cancelWriting, captureWritingTarget, rewriteWriting,
    writingChanges, writingTargetPageId, type WritingAnalysis, type WritingRewriteIssue, type WritingScope, type WritingTarget,
  } from "../lib/writing";

  let { pageId, pageTitle, anchor, preferFocusedPageForPageScope = false, onOpenSettings }: {
    pageId: string;
    pageTitle: string;
    anchor: CurrentBlockAnchor | null;
    preferFocusedPageForPageScope?: boolean;
    onOpenSettings: () => void;
  } = $props();

  let scope = $state<WritingScope>("block");
  let health = $state<HealthStatus | null>(null);
  let checking = $state(true);
  let error = $state("");
  let status = $state("");
  let progress = $state("");
  let result = $state<WritingAnalysis | null>(null);
  let resultTarget = $state<WritingTarget | null>(null);
  let skipped = $state<WritingRewriteIssue[]>([]);
  let operation = $state<{ id: string; kind: "analyze" | "rewrite"; applying: boolean } | null>(null);
  let cancelling = $state(false);
  let generation = 0;
  const context = $derived({ pageId, anchor, preferFocusedPageForPageScope });
  const targetPageId = $derived(writingTargetPageId(context));
  const ready = $derived(health?.enabled && health?.llm_available);
  const hasTarget = $derived(!!targetPageId && (scope === "page" || (anchor?.pageId === targetPageId && !!anchor.blockId)));

  onMount(() => {
    void refreshHealth();
    const refresh = () => { void refreshHealth(); };
    window.addEventListener("ai-configuration-changed", refresh);
    return () => window.removeEventListener("ai-configuration-changed", refresh);
  });

  async function refreshHealth() {
    checking = true;
    try {
      health = await aiHealthCheck();
    } catch (cause) {
      health = null;
      error = `Could not check AI connection: ${String(cause)}`;
    } finally {
      checking = false;
    }
  }

  $effect(() => {
    // Reading these keys clears a result when its source selection changes.
    targetPageId;
    const blockId = scope === "block" ? anchor?.blockId : null;
    void blockId;
    result = null;
    resultTarget = null;
    skipped = [];
    status = "";
    generation++;
    untrack(() => {
      if (operation && !operation.applying) void cancel();
    });
  });

  onDestroy(() => {
    generation++;
    if (operation && !operation.applying) {
      void cancelWriting(operation.id).catch((cause: unknown) => console.error("Could not cancel writing analysis:", cause));
    }
  });

  async function cancel() {
    if (!operation || operation.applying || cancelling) return;
    cancelling = true;
    generation++;
    try {
      await cancelWriting(operation.id);
      status = "Cancelled. No writing was changed.";
    } catch (cause) {
      error = `Could not cancel: ${String(cause)}`;
    }
  }

  async function run(kind: "analyze" | "rewrite") {
    if (operation || !ready || !hasTarget) return;
    const id = crypto.randomUUID();
    const myGeneration = ++generation;
    const isCurrent = () => generation === myGeneration;
    operation = { id, kind, applying: false };
    cancelling = false;
    error = "";
    status = "";
    skipped = [];
    progress = kind === "analyze" ? "Reading the selected writing..." : "Preparing a natural rewrite...";
    let unlisten: (() => void) | undefined;
    try {
      unlisten = await listen<{ operationId: string; message: string }>("ai-writing-progress", ({ payload }) => {
        if (payload.operationId === id && isCurrent()) progress = payload.message;
      });
      const target = await captureWritingTarget(context, scope);
      if (!isCurrent()) return;
      progress = `${kind === "analyze" ? "Analyzing" : "Rewriting"} ${target.blocks.length} text block${target.blocks.length === 1 ? "" : "s"} in ${target.pageTitle}...`;
      if (kind === "analyze") {
        const analysis = await analyzeWriting(target.blocks, id);
        if (!isCurrent()) return;
        result = analysis;
        resultTarget = target;
      } else {
        const rewritten = await rewriteWriting(target.blocks, id);
        if (!isCurrent()) return;
        const changes = writingChanges(target.blocks, rewritten.blocks);
        if (!changes.length) {
          skipped = rewritten.skipped;
          status = skipped.length
            ? "No safe wording changes were available. Nothing was changed."
            : "The model kept the original wording. Nothing was changed.";
          return;
        }
        await runUndoOperation(() => withPageEditorsLocked(target.pageId, async () => {
          await flushPageEditors(target.pageId);
          if (!isCurrent()) return;
          if (operation) operation.applying = true;
          progress = "Applying the rewrite...";
          await applyWritingChanges(target.graphPath, target.pageId, changes, target.snapshot);
          const action = { type: "rewrite_writing" as const, graphPath: target.graphPath, pageId: target.pageId, changes };
          pushUndo(action);
          await notifyWritingChanged(action);
          if (isCurrent()) {
            result = null;
            resultTarget = null;
            skipped = rewritten.skipped;
            status = `Rewrote ${changes.length} block${changes.length === 1 ? "" : "s"} in ${target.pageTitle}. Ctrl+Z undoes the whole rewrite.`;
          }
        }));
      }
    } catch (cause) {
      if (isCurrent()) error = String(cause);
    } finally {
      unlisten?.();
      if (operation?.id === id) {
        operation = null;
        progress = "";
        cancelling = false;
      }
    }
  }
</script>

<section class="writing-panel" aria-label="AI writing assessment">
  <h3>Writing assistance</h3>
  <p class="disclaimer">A style estimate from your connected model, not proof of AI authorship. Human writing can score highly; AI writing can score low.</p>

  {#if checking}
    <p role="status">Checking AI connection...</p>
  {:else if !ready}
    <div class="connection-notice">
      <p>Configure a connected AI model to analyze or rewrite writing.</p>
      <button class="primary" onclick={onOpenSettings}>Configure AI</button>
    </div>
  {:else}
    <div class="scope" role="group" aria-label="Writing scope">
      <span>Scope</span>
      <button class:chosen={scope === "block"} aria-pressed={scope === "block"} disabled={!!operation} onclick={() => scope = "block"}>Block</button>
      <button class:chosen={scope === "page"} aria-pressed={scope === "page"} disabled={!!operation} onclick={() => scope = "page"}>Page</button>
    </div>
    <p class="target-hint">
      {#if !hasTarget}Click a block in the editor or open a page first.
      {:else if preferFocusedPageForPageScope}
        {scope === "page" ? "Only the selected journal day, never the entire journal." : "Only the selected block. Child blocks are unchanged."}
      {:else}{scope === "page" ? `Text on ${pageTitle || "this page"}.` : "Only the selected block. Child blocks are unchanged."}{/if}
    </p>
    <div class="actions">
      <button class="primary" disabled={!!operation || !hasTarget} onclick={() => run("analyze")}>Analyze AI style</button>
      <button disabled={!!operation || !hasTarget} onclick={() => run("rewrite")}>Rewrite naturally</button>
    </div>
    <p class="disclaimer">AI edits wording around protected numbers, citations, and formatting. Rejected edits leave the original wording unchanged and are listed below. Review meaning and scientific details; Ctrl+Z undoes the change. No guarantee of a lower detector score. The configured model receives the selected text, whether embedded, on a model server, or a cloud service.</p>
  {/if}

  {#if operation}
    <div class="progress" role="status">
      <span>{cancelling ? "Cancelling..." : progress}</span>
      <button onclick={cancel} disabled={cancelling || operation.applying}>Cancel</button>
    </div>
  {/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if status}<p class="result-status" role="status">{status}</p>{/if}
  {#if skipped.length}
    <section class="result skipped-result" aria-label="Wording left unchanged">
      <h4>Wording left unchanged</h4>
      <p>No safe edit was available for the parts listed here. Their original text was kept.</p>
      {#each skipped as issue}
        <article class="finding">
          <strong>Text block {issue.blockOrdinal}{#if issue.lineOrdinal !== null}, line {issue.lineOrdinal}{/if}</strong>
          <p>{issue.reason}</p>
        </article>
      {/each}
    </section>
  {/if}

  {#if result && resultTarget}
    <div class="result">
      <p class="result-target">{resultTarget.pageTitle} - {resultTarget.scope === "block" ? "Block" : "Page"}</p>
      {#if result.score !== null}
        <div class="score">
          <svg viewBox="0 0 120 120" role="img" aria-label={`AI-like style score: ${result.score} out of 100. Not an authorship probability.`}>
            <circle class="score-track" cx="60" cy="60" r="48" />
            <circle class="score-value" cx="60" cy="60" r="48" pathLength="100"
              stroke-dasharray={`${result.score} 100`} transform="rotate(-90 60 60)" />
            <text x="60" y="59" class="score-number">{result.score}</text>
            <text x="60" y="78" class="score-total">out of 100</text>
          </svg>
          <strong>AI-like style estimate</strong>
          <span>Higher means more patterns the model considers formulaic.</span>
        </div>
      {:else}<strong class="inconclusive">Inconclusive</strong>{/if}
      <p>{result.summary}</p>
      <p class="coverage">{result.analyzedWordCount} of {result.wordCount} words assessed across {result.chunksAnalyzed} of {result.chunksTotal} sections.</p>
      {#if resultTarget.skippedBlocks}<p class="coverage">{resultTarget.skippedBlocks} empty or unsupported blocks excluded.</p>{/if}
      {#if result.findings.length}
        <h4>Patterns noticed</h4>
        {#each result.findings as finding}
          <article class="finding">
            <strong>{finding.label}</strong>
            <p>{finding.detail}</p>
            {#if finding.quote}<blockquote>{finding.quote}</blockquote>{/if}
          </article>
        {/each}
      {/if}
    </div>
  {/if}
</section>

<style>
  .writing-panel { display: flex; flex-direction: column; gap: 12px; color: var(--text-primary); font-size: 13px; }
  h3, h4, p { margin: 0; }
  h3 { font-size: 16px; }
  p { line-height: 1.5; }
  .disclaimer, .target-hint, .coverage { color: var(--text-muted); font-size: 12px; }
  .scope, .actions, .progress { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; }
  .scope span { margin-right: 4px; }
  button { border: 1px solid var(--border); border-radius: 6px; padding: 7px 10px; background: var(--bg-secondary); color: var(--text-primary); cursor: pointer; font: inherit; }
  button:hover:not(:disabled) { background: var(--bg-hover); }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  button:disabled { opacity: 0.5; cursor: not-allowed; }
  button.primary, button.chosen { border-color: var(--accent); color: var(--accent); }
  .actions button { flex: 1 0 auto; }
  .connection-notice, .result, .finding { padding: 12px; background: var(--bg-secondary); border: 1px solid var(--border); border-radius: 8px; }
  .connection-notice button { margin-top: 10px; }
  .result { display: flex; flex-direction: column; gap: 12px; }
  .result-target { color: var(--text-secondary); overflow-wrap: anywhere; }
  .score { display: flex; align-items: center; flex-direction: column; gap: 5px; text-align: center; }
  .score svg { width: 132px; height: 132px; }
  .score circle { fill: none; stroke-width: 10; }
  .score-track { stroke: var(--border); }
  .score-value { stroke: var(--accent); stroke-linecap: round; }
  .score text { text-anchor: middle; fill: var(--text-primary); }
  .score-number { font-size: 28px; font-weight: 600; }
  .score-total { font-size: 11px; }
  .score span { font-size: 12px; color: var(--text-muted); }
  .finding p { margin-top: 6px; }
  blockquote { margin: 10px 0 0; padding-left: 10px; border-left: 2px solid var(--accent); color: var(--text-secondary); white-space: pre-wrap; overflow-wrap: anywhere; }
  .error { color: var(--danger, var(--text-primary)); overflow-wrap: anywhere; }
  .result-status, .inconclusive { color: var(--text-secondary); }
</style>
