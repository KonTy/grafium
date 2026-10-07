<script lang="ts">
  import { save } from "@tauri-apps/plugin-dialog";
  import { dialogKeydown, dismissOnBackdrop } from "../lib/modal";
  import SettingsHelp from "./SettingsHelp.svelte";
  import {
    buildRequestedDocument,
    loadPrintSource,
    mountPrintDocument,
    printHeadings,
    requestedBlocks,
    sendToPrinter,
    systemDialogSavesPdf,
    unmountPrintDocument,
    type PrintColour,
    type PrintSource,
  } from "../lib/printing";
  import type { PrintScopeKind } from "../lib/printScope";

  let { pageId, onClose }: { pageId: string; onClose: () => void } = $props();

  let source = $state<PrintSource | null>(null);
  let scope = $state<PrintScopeKind>("page");
  let chapterId = $state<string | null>(null);
  let colour = $state<PrintColour>("colour");
  let busy = $state(false);
  let error = $state("");

  $effect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const loaded = await loadPrintSource(pageId);
        if (cancelled) return;
        source = loaded;
        // Start on whatever the user has already shown interest in.
        chapterId = loaded.suggestedChapterId ?? loaded.chapters[0]?.id ?? null;
        scope = loaded.selectedIds.length > 0 ? "selection" : "page";
      } catch (cause) {
        if (!cancelled) error = `Could not read this page: ${String(cause)}`;
      }
    })();
    return () => {
      cancelled = true;
    };
  });

  const request = $derived({ pageId, scope, chapterId, colour });
  const documentHtml = $derived(source ? buildRequestedDocument(source, request) : "");
  const headings = $derived(source ? printHeadings(source, request) : null);
  const hasContent = $derived(source ? requestedBlocks(source, request).length > 0 : false);
  // Android's own print dialog lists "Save as PDF" among the destinations, so
  // offering a separate button there would be a second door to one room.
  const systemPdf = systemDialogSavesPdf();

  // Closing stays available while a job runs: the printer owns the job from
  // here, and a job that never reports back must not trap the user in a modal.
  function close() {
    onClose();
  }

  /** Keep a page title usable as a file name. */
  function pdfFileName(title: string): string {
    const cleaned = title.replace(/[\\/:*?"<>|]/g, " ").trim();
    return `${cleaned || "Grafium page"}.pdf`;
  }

  async function run(action: "printer" | "pdf") {
    if (busy || !source || !hasContent) return;
    busy = true;
    error = "";
    try {
      let pdfPath: string | undefined;
      if (action === "pdf" && !systemPdf) {
        const chosen = await save({
          defaultPath: pdfFileName(headings?.title ?? source.pageTitle),
          filters: [{ name: "PDF", extensions: ["pdf"] }],
        });
        if (!chosen) return;
        pdfPath = chosen;
      }
      mountPrintDocument(documentHtml, colour);
      const outcome = await sendToPrinter(action, pdfPath, headings?.title ?? source.pageTitle);
      if (outcome !== "cancelled") onClose();
    } catch (cause) {
      error = `Could not print: ${String(cause)}`;
    } finally {
      // The document is only ever meant to exist for the length of one job.
      unmountPrintDocument();
      busy = false;
    }
  }
</script>

<div class="print-backdrop" role="presentation" use:dismissOnBackdrop={close}>
  <div
    class="print-dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="print-dialog-title"
    data-help-context="printing"
    tabindex="-1"
    onkeydown={dialogKeydown(close)}
  >
    <div class="title-row">
      <h2 id="print-dialog-title">Print</h2>
      <SettingsHelp title="Printing">
        <p>
          Grafium prints from your saved notes rather than from the screen, so folded
          sections, long pages and anything scrolled out of view all come out in full.
        </p>
        <p>
          Printed pages are always white with dark text, whatever theme you use, so a dark
          theme never floods the page with ink. Choose <strong>Black and white</strong> to
          drop colour from the text and pictures as well.
        </p>
        <p>
          <strong>Save as PDF</strong> produces exactly the same document as the printer
          would, written straight to a file you choose.
        </p>
      </SettingsHelp>
    </div>

    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}

    {#if !source}
      <p class="loading">Reading this page…</p>
    {:else}
      <fieldset>
        <legend>What to print</legend>
        <label class="choice">
          <input type="radio" bind:group={scope} value="page" />
          <span>This whole page</span>
        </label>
        {#if source.chapters.length > 0}
          <label class="choice">
            <input type="radio" bind:group={scope} value="chapter" />
            <span>One chapter</span>
          </label>
          {#if scope === "chapter"}
            <select bind:value={chapterId} aria-label="Chapter to print">
              {#each source.chapters as chapter (chapter.id)}
                <option value={chapter.id}>
                  {"  ".repeat(chapter.level - 1)}{chapter.text}
                </option>
              {/each}
            </select>
          {/if}
        {/if}
        {#if source.selectedIds.length > 0}
          <label class="choice">
            <input type="radio" bind:group={scope} value="selection" />
            <span>
              The {source.selectedIds.length} selected
              {source.selectedIds.length === 1 ? "block" : "blocks"}
            </span>
          </label>
        {/if}
      </fieldset>

      <fieldset>
        <legend>Ink</legend>
        <label class="choice">
          <input type="radio" bind:group={colour} value="colour" />
          <span>Colour</span>
        </label>
        <label class="choice">
          <input type="radio" bind:group={colour} value="mono" />
          <span>Black and white</span>
        </label>
        <p class="note">Paper is always white with dark text, whichever you pick.</p>
        {#if systemPdf}
          <p class="note">Choose <strong>Save as PDF</strong> in the print window to make a file.</p>
        {/if}
      </fieldset>

      <div class="preview-wrap">
        <span class="preview-label">Preview</span>
        <div class="preview-sheet" class:preview-mono={colour === "mono"} aria-hidden="true">
          {@html documentHtml}
        </div>
      </div>
    {/if}

    <div class="actions">
      <button type="button" onclick={close}>Cancel</button>
      {#if !systemPdf}
        <button type="button" onclick={() => void run("pdf")} disabled={busy || !hasContent}>
          Save as PDF…
        </button>
      {/if}
      <button
        type="button"
        class="primary"
        onclick={() => void run("printer")}
        disabled={busy || !hasContent}
      >
        {busy ? "Working…" : "Print…"}
      </button>
    </div>
  </div>
</div>

<style>
  .print-backdrop { position: fixed; inset: 0; z-index: 2000; background: var(--overlay-bg, #0008); display: grid; place-items: center; padding: 16px; }
  .print-dialog { box-sizing: border-box; width: min(620px, 100%); max-height: calc(100dvh - 32px); overflow-y: auto; padding: 24px; border: 1px solid var(--border); border-radius: 10px; background: var(--bg-primary); color: var(--text-primary); box-shadow: 0 12px 40px #0004; }
  .title-row { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin: 0 0 16px; }
  h2 { margin: 0; font-size: 18px; }
  fieldset { margin: 0 0 14px; padding: 12px 14px; border: 1px solid var(--border); border-radius: 6px; }
  legend { padding: 0 6px; font-size: 12px; font-weight: 600; color: var(--text-secondary); }
  .choice { display: flex; align-items: center; gap: 8px; padding: 3px 0; font-size: 14px; cursor: pointer; }
  select { width: 100%; margin: 6px 0 2px; padding: 7px; font: inherit; font-size: 13px; background: var(--bg-secondary); color: var(--text-primary); border: 1px solid var(--border); border-radius: 5px; }
  .note { margin-top: 6px; font-size: 12px; color: var(--text-muted); }
  .loading { font-size: 13px; color: var(--text-secondary); }
  .preview-label { display: block; margin-bottom: 6px; font-size: 12px; font-weight: 600; color: var(--text-secondary); }
  /* The preview is the printed page, so it uses paper colours rather than the
     app theme — that is the point of showing it. */
  .preview-sheet { max-height: 230px; overflow: auto; padding: 14px 16px; border: 1px solid var(--border); border-radius: 6px; background: #ffffff; color: #111111; font-family: Georgia, "Times New Roman", serif; font-size: 11px; line-height: 1.45; }
  .preview-sheet :global(.print-title) { font-size: 16px; }
  .preview-sheet :global(.print-subtitle) { font-size: 10px; color: #555; }
  .preview-sheet :global(.print-header) { margin-bottom: 10px; padding-bottom: 5px; border-bottom: 1px solid #ccc; }
  .preview-sheet :global(.print-block) { padding-left: calc(var(--print-depth, 0) * 12px); margin-bottom: 4px; }
  .preview-sheet :global(.print-block-bulleted) > :global(.print-block-content) { position: relative; padding-left: 12px; }
  .preview-sheet :global(.print-block-bulleted) > :global(.print-block-content)::before { content: "•"; position: absolute; left: 0; }
  .preview-sheet :global(a) { color: #1a4f9c; }
  .preview-sheet :global(img) { max-width: 100%; height: auto; }
  .preview-sheet :global(pre), .preview-sheet :global(code) { white-space: pre-wrap; word-break: break-word; font-size: 10px; }
  .preview-sheet :global(table) { width: 100%; border-collapse: collapse; }
  .preview-sheet :global(th), .preview-sheet :global(td) { border: 1px solid #ccc; padding: 2px 4px; }
  .preview-mono :global(*) { color: #000 !important; border-color: #000 !important; background-color: transparent !important; }
  .preview-mono :global(img) { filter: grayscale(100%); }
  button { font: inherit; font-size: 13px; padding: 8px 12px; border: 1px solid var(--border); border-radius: 5px; background: var(--btn-bg); color: var(--text-primary); cursor: pointer; }
  button:hover:not(:disabled) { background: var(--bg-hover); }
  button:disabled { cursor: default; opacity: .55; }
  button:focus-visible, select:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .actions { display: flex; gap: 8px; flex-wrap: wrap; justify-content: flex-end; margin-top: 18px; }
  .primary { color: var(--btn-primary-fg); background: var(--btn-primary-bg); border-color: transparent; }
  .primary:hover:not(:disabled) { background: var(--btn-primary-hover); }
  .error { margin-bottom: 10px; font-size: 13px; color: var(--danger, var(--text-primary)); overflow-wrap: anywhere; }
</style>
