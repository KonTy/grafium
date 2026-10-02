<script lang="ts">
  import { tick } from "svelte";
  import { listAssetTrash, openAssetTrashContainingFolder, purgeTrashedAssets, restoreTrashedAssets } from "../lib/api";
  import type { AssetTrashEntry, AssetTrashScan } from "../lib/api";
  import { dialogKeydown } from "../lib/modal";

  type Action = "restore" | "purge";
  let scan = $state<AssetTrashScan | null>(null);
  let selected = $state<string[]>([]);
  let loading = $state(false);
  let working = $state(false);
  let openingFolder = $state(false);
  let scanValid = $state(false);
  let error = $state("");
  let results = $state<{ graphPath: string; action: Action; confirmed: AssetTrashEntry[]; errors: string[] }[]>([]);
  let pending = $state<{ graphPath: string; assets: AssetTrashEntry[]; action: Action } | null>(null);
  let dialog = $state<HTMLDialogElement>();
  let cancelButton = $state<HTMLButtonElement>();
  let refreshButton = $state<HTMLButtonElement>();
  let trigger: HTMLElement | null = null;
  const componentId = $props.id();
  const descriptionId = `${componentId}-trash-description`;
  const busy = $derived(loading || working || openingFolder);
  const selectedAssets = $derived(scan?.assets.filter((asset) => selected.includes(asset.trash_filename)) ?? []);
  const unavailable = $derived(busy || !!pending || !scanValid);

  function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
  }

  async function loadTrash() {
    if (busy || pending) return;
    loading = true;
    scanValid = false;
    error = "";
    try {
      scan = await listAssetTrash();
      selected = [];
      scanValid = true;
    } catch (e) {
      error = `Trash list failed: ${String(e)}. Refresh trash before continuing.`;
    } finally {
      loading = false;
    }
  }

  async function openContainingFolder(asset: AssetTrashEntry) {
    if (!scan || unavailable) return;
    openingFolder = true;
    error = "";
    try {
      await openAssetTrashContainingFolder(scan.graph_path, asset);
    } catch (e) {
      error = `Could not open containing folder: ${String(e)}. Refresh trash if the file was moved or removed.`;
    } finally {
      openingFolder = false;
    }
  }

  async function review(action: Action, assets: AssetTrashEntry[]) {
    if (!scan || unavailable || !assets.length) return;
    trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    pending = { graphPath: scan.graph_path, assets: assets.map((asset) => ({ ...asset })), action };
    await tick();
    dialog?.showModal();
    cancelButton?.focus({ preventScroll: true });
  }

  function closeConfirmation() {
    if (working) return;
    dialog?.close();
    pending = null;
    void tick().then(() => {
      if (trigger?.isConnected && !trigger.matches(":disabled")) trigger.focus({ preventScroll: true });
      else refreshButton?.focus({ preventScroll: true });
    });
  }

  async function applyAction() {
    if (!pending || busy) return;
    const request = pending;
    working = true;
    error = "";
    try {
      const result = await (request.action === "restore" ? restoreTrashedAssets : purgeTrashedAssets)(
        request.graphPath, request.assets,
      );
      const ids = new Set(request.action === "restore" ? result.restored : result.purged);
      const confirmed = request.assets.filter((asset) => ids.has(asset.trash_filename));
      results = [...results, { graphPath: request.graphPath, action: request.action, confirmed, errors: result.errors }];
      // Keep unconfirmed entries, including failures, in their graph-bound preview.
      const removed = new Set(confirmed.map((asset) => asset.trash_filename));
      if (scan) scan = { ...scan, assets: scan.assets.filter((asset) => !removed.has(asset.trash_filename)) };
      selected = selected.filter((id) => !removed.has(id));
    } catch (e) {
      error = `${request.action === "restore" ? "Restore" : "Permanent deletion"} failed: ${String(e)}. No changes were confirmed; refresh trash to check the current files.`;
      scanValid = false;
    } finally {
      working = false;
      closeConfirmation();
    }
  }
</script>

<section class="asset-trash" data-help-context="settings" aria-label="Asset trash">
  <h2>Asset trash</h2>
  <p>Deleted attachments and manual cleanup use the same persistent, device-local graph trash, which is never synced. Undo restores deleted text or blocks and their attachments while the files remain in trash. Restore below recovers files only; it does not recreate deleted notes.</p>
  <p>Never overwrite an existing original: Restore refuses that path and keeps the trash copy. Trash is never automatically purged. <strong>Permanently delete frees disk space, but Undo cannot recover permanently deleted attachments.</strong></p>
  <p>If Undo reports multiple versions of an attachment, review each copy's trash path (including its batch ID), size, and SHA-256 fingerprint. Select the intended version, Restore it, then retry Undo. Other copies remain in trash; Grafium does not silently substitute them for a purged version.</p>
  <button bind:this={refreshButton} onclick={loadTrash} disabled={busy || !!pending}>
    {loading ? "Loading trash…" : scan ? "Refresh trash" : "List trash"}
  </button>
  {#if error}<p role="alert" class="error">{error}</p>{/if}
  {#if scan}
    <p>Trash preview for graph: <code>{scan.graph_path}</code></p>
    {#if scan.assets.length === 0}
      <p role="status">No files remain in this trash preview.</p>
    {:else}
      <p>{scan.assets.length} trashed file{scan.assets.length === 1 ? "" : "s"} ({formatBytes(scan.assets.reduce((total, asset) => total + asset.size, 0))} total)</p>
      <div class="actions">
        <label>
          <input type="checkbox" aria-label="Select all trash"
            checked={selected.length === scan.assets.length}
            indeterminate={selected.length > 0 && selected.length < scan.assets.length}
            disabled={unavailable}
            onchange={(event) => { selected = event.currentTarget.checked ? scan!.assets.map((asset) => asset.trash_filename) : []; }} />
          Select all trash
        </label>
        <button disabled={unavailable || !selectedAssets.length} onclick={() => review("restore", selectedAssets)}>Restore selected ({selectedAssets.length})…</button>
        <button disabled={unavailable} onclick={() => review("restore", scan!.assets)}>Restore all {scan.assets.length}…</button>
        <button disabled={unavailable || !selectedAssets.length} onclick={() => review("purge", selectedAssets)}>Permanently delete selected ({selectedAssets.length})…</button>
        <button disabled={unavailable} onclick={() => review("purge", scan!.assets)}>Permanently delete all {scan.assets.length}…</button>
      </div>
      <ul class="trash-list" aria-label="Trashed assets" aria-busy={busy}>
        {#each scan.assets as asset (asset.trash_filename)}
          <li>
            <label>
              <input type="checkbox" bind:group={selected} value={asset.trash_filename}
                aria-label={`Select trash ${asset.trash_filename}`} disabled={unavailable} />
              <span><code>{asset.filename}</code><br /><small>Trash: <code>{asset.trash_filename}</code></small><br /><small>SHA-256: <code>{asset.sha256}</code></small></span>
            </label>
            <span class="size" title={`${asset.size} bytes`}>{formatBytes(asset.size)}</span>
            <button disabled={unavailable} onclick={() => openContainingFolder(asset)}
              title={`Open the folder containing ${asset.trash_filename} in your default file manager`}>Open containing folder</button>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
  {#each results as result}
    <div class="result">
      <p>Graph: <code>{result.graphPath}</code></p>
      <p role="status">{result.action === "restore" ? "Restored" : "Permanently deleted"} {result.confirmed.length} file{result.confirmed.length === 1 ? "" : "s"}.</p>
      {#if result.confirmed.length}
        <ul aria-label="Confirmed trash changes">
          {#each result.confirmed as asset}<li><code>{asset.filename}</code> — <code>{asset.trash_filename}</code></li>{/each}
        </ul>
      {/if}
      {#if result.errors.length}
        <div role="alert" class="error">
          <p>Some files could not be processed. Only confirmed changes were removed from the preview. Review the errors and refresh trash.</p>
          <ul>{#each result.errors as message}<li>{message}</li>{/each}</ul>
        </div>
      {/if}
    </div>
  {/each}
  {#if pending}
    <dialog bind:this={dialog} role="alertdialog" aria-modal="true"
      aria-label={pending.action === "purge" ? "Confirm permanent asset deletion" : "Confirm asset restoration"}
      aria-describedby={descriptionId} onkeydown={dialogKeydown(closeConfirmation)}
      oncancel={(event) => { event.preventDefault(); closeConfirmation(); }}>
      <h2>{pending.action === "purge" ? "Permanently delete" : "Restore"} {pending.assets.length} file{pending.assets.length === 1 ? "" : "s"}?</h2>
      <p>Graph: <code>{pending.graphPath}</code></p>
      <ul class="confirmation-list">
        {#each pending.assets as asset}
          <li><code>{asset.filename}</code> ({formatBytes(asset.size)})<br /><small>Trash: <code>{asset.trash_filename}</code></small><br /><small>SHA-256: <code>{asset.sha256}</code></small></li>
        {/each}
      </ul>
      {#if pending.action === "purge"}
        <p id={descriptionId}><strong>This permanently deletes the selected attachment bytes to free disk space. Undo cannot recover permanently deleted attachments. Undo that needs a purged attachment will fail rather than silently restore broken references. This cannot be undone.</strong> Keep a separate backup if you may need these files.</p>
      {:else}
        <p id={descriptionId}>Restore files to their original graph-relative paths. Existing originals are never overwritten; conflicts stay in trash with an error. This restores files, not deleted notes.</p>
      {/if}
      <div class="actions">
        <button bind:this={cancelButton} disabled={working} onclick={closeConfirmation}>Cancel</button>
        <button disabled={working} onclick={applyAction}>
          {working ? "Working…" : pending.action === "purge" ? "Confirm permanent deletion" : "Confirm restore"}
        </button>
      </div>
    </dialog>
  {/if}
</section>

<style>
  .asset-trash { border-top: 1px solid var(--border); margin-top: 20px; padding-top: 8px; }
  h2 { font-size: 18px; }
  p { margin: 10px 0; line-height: 1.5; }
  code, .error { overflow-wrap: anywhere; }
  button { padding: 7px 12px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg-secondary); color: var(--text-primary); cursor: pointer; font: inherit; }
  button:hover:not(:disabled) { background: var(--bg-hover); }
  button:disabled { opacity: .5; cursor: not-allowed; }
  button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .actions { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  label { display: flex; gap: 8px; align-items: center; min-width: 0; }
  .trash-list { padding: 0; list-style: none; max-height: 320px; overflow-y: auto; border: 1px solid var(--border); border-radius: 6px; }
  .trash-list li { display: flex; gap: 8px; align-items: center; justify-content: space-between; flex-wrap: wrap; padding: 8px; border-bottom: 1px solid var(--border); }
  .trash-list li label { flex: 1 1 240px; }
  .trash-list li:last-child { border-bottom: none; }
  .size { white-space: nowrap; }
  .error { color: var(--danger); }
  .result { border-top: 1px solid var(--border); margin-top: 12px; }
  dialog { box-sizing: border-box; margin: auto; width: min(560px, calc(100vw - 32px)); max-height: calc(100dvh - 32px); overflow-y: auto; padding: 24px; border: 1px solid var(--border); border-radius: 10px; background: var(--bg-secondary); color: var(--text-primary); box-shadow: 0 16px 64px rgb(0 0 0 / .4); }
  dialog::backdrop { background: rgb(0 0 0 / .65); }
  .confirmation-list { max-height: 220px; overflow-y: auto; padding-left: 20px; }
</style>
