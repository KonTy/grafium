<script lang="ts">
  import { onMount } from "svelte";
  import { autofocus } from "../lib/autofocus";
  import { dismissOnBackdrop, dialogKeydown } from "../lib/modal";
  import {
    getSyncConflictState, resolveSyncConflict, summarizeSyncResult,
    type SyncConflict, type SyncConflictState, type SyncConflictSide,
  } from "../lib/sync";

  let { conflict, onClose, onResolved }: {
    conflict: SyncConflict;
    onClose: () => void;
    onResolved: () => void;
  } = $props();
  let snapshot = $state<SyncConflictState | null>(null);
  let chosen = $state<SyncConflictSide | null>(null);
  let busy = $state(false);
  let error = $state("");
  let resolved = $state(false);
  let disposed = false;
  let generation = 0;

  function close() {
    if (!busy) {
      if (resolved) onResolved();
      else onClose();
    }
  }

  async function refresh(targetId = conflict.target_id, relPath = conflict.rel_path) {
    const request = ++generation;
    busy = true;
    snapshot = null;
    chosen = null;
    error = "";
    resolved = false;
    try {
      const next = await getSyncConflictState(targetId, relPath);
      if (!disposed && request === generation) snapshot = next;
    } catch (cause) {
      if (!disposed && request === generation) error = `Could not read both versions: ${String(cause)}`;
    } finally {
      if (!disposed && request === generation) busy = false;
    }
  }

  async function applyChoice() {
    if (!snapshot || !chosen || busy) return;
    const request = generation;
    busy = true;
    error = "";
    try {
      const result = await resolveSyncConflict(conflict.target_id, snapshot, chosen);
      if (disposed || request !== generation) return;
      if (result.errors.length || result.conflicts.length) {
        resolved = result.conflicts.length === 0;
        error = `${summarizeSyncResult(result)}. ${result.errors.join("; ")}`;
        if (resolved) error += " The file choice was applied, but indexing needs attention.";
      } else {
        onResolved();
      }
    } catch (cause) {
      // Never retry a stale choice automatically: fetch and confirm anew.
      if (!disposed && request === generation) {
        error = `Nothing further will be overwritten. ${String(cause)}. Reload versions and choose again.`;
        snapshot = null;
        chosen = null;
      }
    } finally {
      if (!disposed && request === generation) busy = false;
    }
  }

  $effect(() => {
    void refresh(conflict.target_id, conflict.rel_path);
  });
  onMount(() => () => { disposed = true; });
</script>

<div class="sync-resolution-backdrop" role="presentation" use:dismissOnBackdrop={close}>
  <div class="sync-resolution-dialog" role="dialog" aria-modal="true"
    aria-labelledby="sync-resolution-title" data-help-context="sync" tabindex="-1"
    onkeydown={dialogKeydown(close)} use:autofocus>
    <h2 id="sync-resolution-title">Resolve sync conflict</h2>
    <p><strong>{conflict.rel_path}</strong><br />Target: {conflict.target_name}</p>
    <p>Choose the current local or remote version, including a deletion. This replaces the other
      side only if both versions still match this view. File bytes are kept exactly, including for books and images.</p>
    <p>Recovery copies remain in this graph's metadata folder under <code>sync-recovery/</code>,
      outside normal notes and sync.
      Filesystem targets cannot exclude changes made at the same instant by another application;
      WebDAV overwrites require a server ETag.</p>
    {#if snapshot && !resolved}
      <fieldset disabled={busy}>
        <legend>Version to keep</legend>
        <label><input type="radio" name="sync-side" value="local" bind:group={chosen} />
          Local: {snapshot.local_hash === null ? "deleted" : `${snapshot.local_size} bytes`}</label>
        <label><input type="radio" name="sync-side" value="remote" bind:group={chosen} />
          Remote: {snapshot.remote_hash === null ? "deleted" : `${snapshot.remote_size} bytes`}</label>
      </fieldset>
      {#if chosen}
        <p class="warning" role="status">
          {#if snapshot[`${chosen}_hash`] === null}
            Confirm deletion of the {chosen === "local" ? "remote" : "local"} file.
          {:else}
            Confirm replacing the {chosen === "local" ? "remote" : "local"} file with this {chosen} version.
          {/if}
          Both available versions are retained as recovery copies.
        </p>
      {/if}
    {/if}
    {#if busy}<p role="status">Checking versions…</p>{/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <div class="actions">
      <button onclick={close} disabled={busy}>{resolved ? "Close" : "Cancel"}</button>
      {#if !resolved}
        <button onclick={() => refresh()} disabled={busy}>Reload versions</button>
        <button onclick={applyChoice} disabled={busy || !snapshot || !chosen}>Confirm choice</button>
      {/if}
    </div>
  </div>
</div>

<style>
  .sync-resolution-backdrop { position: fixed; inset: 0; z-index: 2000; background: var(--overlay-bg, #0008); display: grid; place-items: center; padding: 16px; }
  .sync-resolution-dialog { box-sizing: border-box; width: min(560px, 100%); max-height: calc(100dvh - 32px); overflow: auto; padding: 24px; border: 1px solid var(--border); border-radius: 10px; background: var(--bg-primary); color: var(--text-primary); }
  h2 { margin: 0 0 16px; font-size: 18px; }
  p { font-size: 13px; line-height: 1.5; overflow-wrap: anywhere; }
  fieldset { margin: 16px 0; border: 1px solid var(--border); }
  label { display: block; padding: 8px 0; }
  .actions { display: flex; justify-content: flex-end; flex-wrap: wrap; gap: 8px; margin-top: 20px; }
  button { padding: 8px 12px; border: 1px solid var(--border); border-radius: 5px; background: var(--btn-bg); color: var(--text-primary); cursor: pointer; }
  button:disabled { opacity: .55; cursor: default; }
  button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .warning { font-weight: 600; }
  .error { color: var(--danger, var(--text-primary)); }
</style>
