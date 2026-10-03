<script lang="ts">
  import { autofocus } from "../lib/autofocus";
  import { dismissOnBackdrop, dialogKeydown } from "../lib/modal";
  import { queueBookImport, type BookImportMode } from "../lib/bookImport";
  import SettingsHelp from "./SettingsHelp.svelte";

  let { onChooseFile, onChooseFolder, onClose, onQueued }: {
    onChooseFile: (mode: BookImportMode) => Promise<string | null>;
    onChooseFolder: () => Promise<string | null>;
    onClose: () => void;
    onQueued: () => void;
  } = $props();
  let convert = $state(false);
  const mode = $derived<BookImportMode>(convert ? "markdown" : "original");
  let sourcePath = $state("");
  let busy = $state(false);
  let error = $state("");

  function close() {
    if (!busy) onClose();
  }

  async function choose(kind: "file" | "folder") {
    busy = true;
    error = "";
    try {
      const path = kind === "file" ? await onChooseFile(mode) : await onChooseFolder();
      if (path) sourcePath = path;
    } catch (cause) {
      error = `Could not choose the book source: ${String(cause)}`;
    } finally {
      busy = false;
    }
  }

  async function submit() {
    if (busy || !sourcePath.trim()) return;
    busy = true;
    error = "";
    try {
      await queueBookImport(mode, sourcePath);
      onQueued();
    } catch (cause) {
      error = `Could not start book import: ${String(cause)}`;
    } finally {
      busy = false;
    }
  }
</script>

<div class="book-import-backdrop" role="presentation" use:dismissOnBackdrop={close}>
  <div class="book-import-dialog" role="dialog" aria-modal="true" aria-labelledby="book-import-title"
    data-help-context="books" tabindex="-1" onkeydown={dialogKeydown(close)}>
    <div class="title-row">
      <h2 id="book-import-title">Import books</h2>
      <SettingsHelp title="Importing books">
        <p>Without conversion, Grafium copies EPUB, FB2, MOBI, AZW3, or PDF into this
          graph and you read it under Books. The original stays unchanged; extracted text
          is indexed for search and AI. Notes are saved in one adjacent JSON-LD file
          (1.epub → 1.jsonld). An existing matching Grafium annotation file is imported too.
          DRM-protected ebooks are not supported.</p>
        <p><strong>Convert to editable Markdown</strong> turns supported books and documents
          into editable Markdown pages under Books instead. The original stays outside the
          graph. Conversion may change layout and formatting.</p>
        <p>Folder imports scan subfolders. Follow progress and indexing warnings in
          Background jobs.</p>
      </SettingsHelp>
    </div>
    <label class="conversion">
      <input type="checkbox" bind:checked={convert} disabled={busy} use:autofocus />
      Convert to editable Markdown
    </label>
    <label for="book-import-source">Book file or folder</label>
    <input id="book-import-source" type="text" bind:value={sourcePath} disabled={busy}
      placeholder="/path/to/book.epub or /path/to/books"
      onkeydown={(event) => { if (event.key === "Enter") { event.preventDefault(); void submit(); } }} />
    <div class="source-actions">
      <button onclick={() => choose("file")} disabled={busy}>Choose file</button>
      <button onclick={() => choose("folder")} disabled={busy}>Choose folder</button>
    </div>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <div class="actions">
      <button onclick={close} disabled={busy}>Cancel</button>
      <button class="primary" onclick={submit} disabled={busy || !sourcePath.trim()}>
        {busy ? "Please wait..." : mode === "original" ? "Add to Books" : "Convert and import"}
      </button>
    </div>
  </div>
</div>

<style>
  .book-import-backdrop { position: fixed; inset: 0; z-index: 2000; background: var(--overlay-bg, #0008); display: grid; place-items: center; padding: 16px; }
  .book-import-dialog { box-sizing: border-box; width: min(540px, 100%); max-height: calc(100dvh - 32px); overflow-y: auto; padding: 24px; border: 1px solid var(--border); border-radius: 10px; background: var(--bg-primary); color: var(--text-primary); box-shadow: 0 12px 40px #0004; }
  .title-row { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin: 0 0 20px; }
  h2 { margin: 0; font-size: 18px; }
  label { display: block; margin: 14px 0 6px; font-size: 13px; font-weight: 600; }
  input { box-sizing: border-box; width: 100%; padding: 9px; font: inherit; font-size: 14px; background: var(--bg-secondary); color: var(--text-primary); border: 1px solid var(--border); border-radius: 5px; }
  .conversion { display: flex; align-items: center; gap: 8px; }
  .conversion input { width: auto; }
  p { font-size: 13px; line-height: 1.5; color: var(--text-secondary); }
  button { font: inherit; font-size: 13px; padding: 8px 12px; border: 1px solid var(--border); border-radius: 5px; background: var(--btn-bg); color: var(--text-primary); cursor: pointer; }
  button:hover:not(:disabled) { background: var(--bg-hover); }
  button:disabled { cursor: default; opacity: .55; }
  button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .source-actions, .actions { display: flex; gap: 8px; flex-wrap: wrap; }
  .source-actions { margin-top: 8px; }
  .actions { justify-content: flex-end; margin-top: 20px; }
  .primary { color: var(--btn-primary-fg); background: var(--btn-primary-bg); border-color: transparent; }
  .primary:hover:not(:disabled) { background: var(--btn-primary-hover); }
  .error { color: var(--danger, var(--text-primary)); overflow-wrap: anywhere; }
</style>
