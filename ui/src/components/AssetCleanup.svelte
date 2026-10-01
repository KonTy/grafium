<script lang="ts">
  import { tick } from "svelte";
  import { findOrphanedAssets, trashAssets } from "../lib/api";
  import type { AssetCleanupResult, AssetCleanupScan, OrphanedAsset } from "../lib/api";
  import { dialogKeydown } from "../lib/modal";
  import AssetTrash from "./AssetTrash.svelte";

  let scan = $state<AssetCleanupScan | null>(null);
  let selected = $state<string[]>([]);
  let scanning = $state(false);
  let moving = $state(false);
  let scanValid = $state(false);
  let error = $state("");
  let results = $state<AssetCleanupResult[]>([]);
  let pending = $state<{ graphPath: string; assets: OrphanedAsset[] } | null>(null);
  let dialog = $state<HTMLDialogElement>();
  let cancelButton = $state<HTMLButtonElement>();
  let scanButton = $state<HTMLButtonElement>();
  let trigger: HTMLElement | null = null;
  const componentId = $props.id();
  const descriptionId = `${componentId}-cleanup-description`;
  const busy = $derived(scanning || moving);
  const selectedAssets = $derived(scan?.assets.filter((asset) => selected.includes(asset.filename)) ?? []);

  function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
  }

  async function scanAssets() {
    if (busy || pending) return;
    scanning = true;
    scanValid = false;
    error = "";
    try {
      scan = await findOrphanedAssets();
      selected = [];
      scanValid = true;
    } catch (e) {
      error = `Scan failed: ${String(e)}. Re-scan before moving files.`;
    } finally {
      scanning = false;
    }
  }

  async function review(assets: OrphanedAsset[]) {
    if (!scan || !scanValid || busy || pending || !assets.length) return;
    trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    pending = { graphPath: scan.graph_path, assets: assets.map((asset) => ({ ...asset })) };
    await tick();
    dialog?.showModal();
    cancelButton?.focus({ preventScroll: true });
  }

  function closeConfirmation() {
    if (moving) return;
    dialog?.close();
    pending = null;
    void tick().then(() => {
      if (trigger?.isConnected && !trigger.matches(":disabled")) trigger.focus({ preventScroll: true });
      else scanButton?.focus({ preventScroll: true });
    });
  }

  async function moveAssets() {
    if (!pending || busy) return;
    const request = pending;
    moving = true;
    error = "";
    try {
      const result = await trashAssets(request.graphPath, request.assets);
      results = [...results, result];
      // Only confirmed moves leave the preview; refused files stay available for review.
      const moved = new Set(result.moved);
      if (scan) scan = { ...scan, assets: scan.assets.filter((asset) => !moved.has(asset.filename)) };
      selected = selected.filter((filename) => !moved.has(filename));
    } catch (e) {
      error = `Move failed: ${String(e)}. No moves were confirmed; re-scan to check the current files.`;
      scanValid = false;
    } finally {
      moving = false;
      closeConfirmation();
    }
  }
</script>

<div class="asset-cleanup" data-help-context="settings">
  <p>Deleting text or blocks automatically moves newly unreferenced attachments to persistent graph-local trash. Undo restores their references and files while the attachments remain in trash. Shared references are kept conservatively. Whole-page attachment cleanup uses the same recoverable trash.</p>
  <p>AI chat history does not keep pages or attachments alive. Quoted links, citations, and conversation context do not prevent cleanup; old chat links may stop opening. Save content as a note if its attachments should be kept.</p>
  <p>Find unreferenced attachments, including ZIP archives, not just images. Review the full graph-relative paths before moving anything.</p>
  <p><strong>Save pending edits first.</strong> Unsaved editor drafts cannot be checked. The scan checks indexed references and saved graph text, including Markdown, JSON-LD notes, and configuration. Conservative filename matches can retain duplicates.</p>
  <p>References inside binary archives or books are not inspected. This is a conservative scan of supported sources, not proof that every possible reference has been found.</p>
  <p>Files move to recoverable graph-local trash, not permanent deletion. Trash is never synced, but removal of the original files will sync. No disk space is freed until you explicitly choose Permanently delete in Asset trash below. Undo cannot recover purged attachments.</p>
  <button bind:this={scanButton} onclick={scanAssets} disabled={busy || !!pending}>
    {scanning ? "Scanning…" : scan ? "Re-scan" : "Scan for orphaned assets"}
  </button>
  {#if error}<p role="alert" class="error">{error}</p>{/if}

  {#if scan}
    <p>Preview for graph: <code>{scan.graph_path}</code></p>
    {#if scan.assets.length === 0}
      <p role="status">No candidates remain in this preview.</p>
    {:else}
      <p>{scan.assets.length} candidate file{scan.assets.length === 1 ? "" : "s"} ({formatBytes(scan.assets.reduce((total, asset) => total + asset.size, 0))} total)</p>
      <div class="actions">
        <label>
          <input type="checkbox" aria-label="Select all candidates"
            checked={selected.length === scan.assets.length}
            indeterminate={selected.length > 0 && selected.length < scan.assets.length}
            disabled={busy || !!pending || !scanValid}
            onchange={(event) => { selected = event.currentTarget.checked ? scan!.assets.map((asset) => asset.filename) : []; }} />
          Select all
        </label>
        <button disabled={busy || !!pending || !scanValid || !selectedAssets.length} onclick={() => review(selectedAssets)}>
          Move selected ({selectedAssets.length}) to trash…
        </button>
        <button disabled={busy || !!pending || !scanValid} onclick={() => review(scan!.assets)}>
          Move all {scan.assets.length} to trash…
        </button>
      </div>
      <ul class="asset-list" aria-label="Asset cleanup candidates" aria-busy={busy}>
        {#each scan.assets as asset (asset.filename)}
          <li>
            <label class="asset-name">
              <input type="checkbox" bind:group={selected} value={asset.filename} aria-label={`Select ${asset.filename}`}
                disabled={busy || !!pending || !scanValid} />
              <code>{asset.filename}</code>
            </label>
            <span class="size" title={`${asset.size} bytes`}>{formatBytes(asset.size)}</span>
            <button aria-label={`Move ${asset.filename} to trash`} disabled={busy || !!pending || !scanValid}
              onclick={() => review([asset])}>Move to trash…</button>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}

  {#each results as result}
    <div class="result">
      <p role="status">Moved {result.moved.length} file{result.moved.length === 1 ? "" : "s"} to trash.</p>
      {#if result.moved.length}
        <ul aria-label="Files moved to trash">{#each result.moved as filename}<li><code>{filename}</code></li>{/each}</ul>
      {/if}
      {#if result.trash_path}
        <p>Recovery directory: <code>{result.trash_path}</code></p>
        <p>To recover, choose List trash or Refresh trash below, then Restore. Never overwrite an existing original: restore refuses conflicts. Failed or partial attempts may leave recovery copies even when the original was not moved. Trash keeps the original folder structure and is never automatically purged.</p>
      {/if}
      {#if result.errors.length}
        <div role="alert" class="error">
          <p>This attempt could not move some files. Only confirmed moves were removed from the preview. Re-scan after reviewing these errors.</p>
          <ul>{#each result.errors as message}<li>{message}</li>{/each}</ul>
        </div>
      {/if}
    </div>
  {/each}

  {#if pending}
    <dialog bind:this={dialog} role="alertdialog" aria-modal="true" aria-label="Confirm asset cleanup"
      aria-describedby={descriptionId} onkeydown={dialogKeydown(closeConfirmation)}
      oncancel={(event) => { event.preventDefault(); closeConfirmation(); }}>
      <h2>Move {pending.assets.length} file{pending.assets.length === 1 ? "" : "s"} to trash?</h2>
      <p>Graph: <code>{pending.graphPath}</code></p>
      <ul class="confirmation-list">
        {#each pending.assets as asset}<li><code>{asset.filename}</code> ({formatBytes(asset.size)})</li>{/each}
      </ul>
      <p id={descriptionId}>Save pending edits before continuing. References and file contents are checked again; changed or referenced files will be refused. Files go to <code>.grafium/asset-trash/</code> with their original paths for recovery through Asset trash below, not permanent deletion.</p>
      <p>The original removals will sync; the trash will not. Disk space is not freed. Restore never overwrites an existing original. Failed or partial attempts may leave recovery copies of files that were not moved. Trash is not automatically purged; permanent deletion requires a separate confirmation and Undo cannot recover those bytes.</p>
      <div class="actions">
        <button bind:this={cancelButton} disabled={moving} onclick={closeConfirmation}>Cancel</button>
        <button disabled={moving} onclick={moveAssets}>{moving ? "Moving…" : "Confirm move to trash"}</button>
      </div>
    </dialog>
  {/if}
  <AssetTrash />
</div>

<style>
  .asset-cleanup { font-size: 13px; color: var(--text-secondary); }
  p { margin: 10px 0; line-height: 1.5; }
  code, .error { overflow-wrap: anywhere; }
  button { padding: 7px 12px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg-secondary); color: var(--text-primary); cursor: pointer; font: inherit; }
  button:hover:not(:disabled) { background: var(--bg-hover); }
  button:disabled { opacity: .5; cursor: not-allowed; }
  button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .actions { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  label { display: flex; gap: 8px; align-items: center; }
  .asset-list { padding: 0; list-style: none; max-height: 320px; overflow-y: auto; border: 1px solid var(--border); border-radius: 6px; }
  .asset-list li { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; padding: 8px; border-bottom: 1px solid var(--border); }
  .asset-list li:last-child { border-bottom: none; }
  .asset-name { flex: 1; min-width: min(200px, 100%); color: var(--text-primary); }
  .size { white-space: nowrap; }
  .error { color: var(--danger); }
  .result { border-top: 1px solid var(--border); margin-top: 12px; }
  dialog { box-sizing: border-box; margin: auto; width: min(560px, calc(100vw - 32px)); max-height: calc(100dvh - 32px); overflow-y: auto; padding: 24px; border: 1px solid var(--border); border-radius: 10px; background: var(--bg-secondary); color: var(--text-primary); box-shadow: 0 16px 64px rgb(0 0 0 / .4); }
  dialog::backdrop { background: rgb(0 0 0 / .65); }
  h2 { font-size: 18px; }
  .confirmation-list { max-height: 220px; overflow-y: auto; padding-left: 20px; }
</style>
