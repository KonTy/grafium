<script lang="ts">
  import { onDestroy, onMount, tick } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { addPrivateLibraryLocation, movePrivateLibraryLocation, privateLibrary, privateLibraryError, readerNative, refreshPrivateLibrary, removePrivateLibraryLocation, type ReaderLocation } from "../lib/privateReader";
  import { libraryLocationName, libraryLocations } from "../lib/libraryLocations";
  import { androidReaderRequest, isAndroidReader, type ReaderVolumeCapabilities } from "../lib/privateReaderAndroid";
  import PrivateReaderVoices from "./PrivateReaderVoices.svelte";
  import { FALLBACK_LIBRARY_INDEX_STATUS, formatLibraryLastRun, libraryIndexSettingsSet, libraryIndexStart, libraryIndexStatus, subscribeLibraryIndexUpdated, type LibraryIndexStatus } from "../lib/libraryIndex";
  import SettingsHelp from "./SettingsHelp.svelte";
  let busy = $state(false);
  let error = $state("");
  let message = $state("");
  let backup = $state("");
  const android = isAndroidReader();
  let capabilities = $state<ReaderVolumeCapabilities | null>(null);
  let indexStatus = $state<LibraryIndexStatus | null>(null);
  let indexBusy = $state(false);
  let indexError = $state("");
  let confirmRebuild = $state(false);
  let stopIndex: (() => void) | undefined;
  let confirmRebuildButton = $state<HTMLButtonElement>();
  let rebuildButton = $state<HTMLButtonElement>();
  async function refreshCapabilities() { capabilities = await androidReaderRequest<ReaderVolumeCapabilities>("capabilities"); }
  async function setVolume(enabled: boolean, key = capabilities?.volume.settings.key ?? "up") {
    await androidReaderRequest("volumeSettings", { enabled, key, gesture: "longPress" });
    await refreshCapabilities();
  }
  async function refreshIndexStatus() { indexStatus = await libraryIndexStatus(); }
  async function runIndex(action: () => Promise<void>) {
    indexBusy = true; indexError = ""; message = "";
    try { await action(); } catch (cause) { indexError = String(cause); }
    finally { indexBusy = false; }
  }
  async function setIndex(enabled = indexStatus?.enabled ?? true, transcribeMedia = indexStatus?.transcribeMedia ?? true) {
    indexStatus = await libraryIndexSettingsSet(enabled, transcribeMedia);
  }

  async function run(action: () => Promise<void>) {
    busy = true; error = ""; message = "";
    try { await action(); } catch (cause) { error = String(cause); }
    finally { busy = false; }
  }
  async function choose() {
    await androidReaderRequest("pickLocation");
    await refreshPrivateLibrary();
  }
  const locations = $derived(libraryLocations($privateLibrary));
  let removing = $state("");
  async function addLocation() {
    const path = await open({ directory: true, multiple: false, title: "Add a Library location" });
    if (typeof path !== "string") return;
    await addPrivateLibraryLocation(path);
    message = `Added ${libraryLocationName(path)}. Originals stay in that folder; nothing is copied into your graph.`;
  }
  async function changeFolder(location: ReaderLocation) {
    const path = await open({ directory: true, multiple: false, defaultPath: location.path,
      title: `Choose where ${libraryLocationName(location.path)} is now` });
    if (typeof path !== "string") return;
    await movePrivateLibraryLocation(location.path, path);
    message = `${libraryLocationName(path)} now uses ${path}. Its items kept their progress and bookmarks.`;
  }
  async function confirmRemove(location: ReaderLocation, index: number) {
    removing = location.path;
    await tick();
    document.getElementById(`location-cancel-${index}`)?.focus();
  }
  async function removeLocation(location: ReaderLocation) {
    await removePrivateLibraryLocation(location.path);
    removing = "";
    message = `Removed ${libraryLocationName(location.path)} from Library. The files in it were not touched.`;
  }
  async function exportBackup() {
    if (android) {
      const result = await androidReaderRequest<{ exported: boolean } | null>("exportState");
      if (result?.exported) message = "Private history exported to your selected file.";
      return;
    }
    const data = await readerNative<string>("export");
    const url = URL.createObjectURL(new Blob([data], { type: "application/json" }));
    const link = document.createElement("a");
    link.href = url; link.download = "grafium-private-reader.json"; link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    message = "Private history export prepared. Store it somewhere private; it contains book titles, positions, and notes.";
  }
  onMount(() => {
    void refreshPrivateLibrary().catch(cause => { error = String(cause); });
    void refreshIndexStatus().catch(cause => { indexError = String(cause); indexStatus = FALLBACK_LIBRARY_INDEX_STATUS; });
    void subscribeLibraryIndexUpdated(status => { indexStatus = status; }, message => { indexError = message; }).then(unlisten => { stopIndex = unlisten; }).catch(cause => { indexError = String(cause); });
    if (!android) return;
    void refreshCapabilities().catch(cause => { error = String(cause); });
    const changed = () => { void refreshCapabilities().catch(cause => { error = String(cause); }); };
    window.addEventListener("focus", changed);
    return () => { window.removeEventListener("focus", changed); stopIndex?.(); };
  });
  onDestroy(() => { stopIndex?.(); });
</script>

<section data-help-context="reader" class="private-settings">
  {#if android}
  <div class="help-row">
    <h3>Library location</h3>
    <SettingsHelp title="Library location">
      <p>Discover EPUBs and audiobooks in an external local folder. An audiobook folder may contain nested disc folders; each loose MP3 in the library root is a separate book.</p>
      <p>Device-local books, listening progress, and automatic bookmarks never enter graph sync or AI.
        Intentionally written journal [[Book title]] notes are ordinary shared graph content.
        Do not choose a cloud-backed folder if you need originals to remain only on this device; external backup and whole-device policies are outside Grafium’s control.</p>
    </SettingsHelp>
  </div>
  <p class="path">{$privateLibrary.libraryPath || "No library selected"}</p>
  <div class="actions"><button disabled={busy} onclick={() => run(choose)}>Choose local folder…</button><button disabled={busy || !$privateLibrary.libraryPath} onclick={() => run(() => refreshPrivateLibrary(true))}>Rescan library</button></div>
  {:else}
  <div class="help-row">
    <h3>Library locations</h3>
    <SettingsHelp title="Library locations">
      <p>Add folders on this computer, external drives, SD cards, or file-server shares mounted as folders. Grafium finds EPUBs, audiobooks, and videos in each. An audiobook folder may contain nested disc folders; each loose audio file is a separate item.</p>
      <p>When a drive is unplugged or a share is not mounted, its items stay in Library, marked Disconnected, with their progress, bookmarks, and search index. Reconnect it and choose Rescan to open them again. If the drive is now mounted at a different path, use Change folder; its items keep their history.</p>
      <p>Device-local books, listening progress, and automatic bookmarks never enter graph sync or AI.
        Intentionally written journal [[Book title]] notes are ordinary shared graph content.
        Do not choose a cloud-backed folder if you need originals to remain only on this device; external backup and whole-device policies are outside Grafium’s control.</p>
    </SettingsHelp>
  </div>
  {#if locations.length}
    <ul class="locations" aria-label="Library locations">
      {#each locations as location, index (location.path)}
        <li>
          <div class="location-head">
            <strong>{libraryLocationName(location.path)}</strong>
            <span class="location-state" class:offline={!location.connected}>{location.connected ? "Connected" : "Disconnected"}</span>
          </div>
          <span class="location-detail">{location.path} · {location.items} {location.items === 1 ? "item" : "items"}{!location.connected && location.reason ? ` · ${location.reason}` : ""}</span>
          {#if location.connected && location.reason}<span class="error">{location.reason}</span>{/if}
          {#if removing === location.path}
            <div class="remove-location" role="group" aria-label={`Confirm removing ${libraryLocationName(location.path)}`}>
              <p class="warning">Remove “{libraryLocationName(location.path)}” from Library?{location.items ? ` Its ${location.items} ${location.items === 1 ? "item leaves Library with its" : "items leave Library with their"} reading progress, bookmarks, and search index.` : ""} The files in the folder are not touched.</p>
              <div class="actions"><button id={`location-cancel-${index}`} disabled={busy} onclick={() => removing = ""}>Cancel</button>
                <button disabled={busy} onclick={() => run(() => removeLocation(location))}>Remove location</button></div>
            </div>
          {:else}
            <div class="actions"><button disabled={busy} onclick={() => run(() => changeFolder(location))}>Change folder…</button>
              <button disabled={busy} onclick={() => confirmRemove(location, index)}>Remove…</button></div>
          {/if}
        </li>
      {/each}
    </ul>
  {:else}
    <p class="path">No Library locations yet</p>
  {/if}
  <div class="actions"><button disabled={busy} onclick={() => run(addLocation)}>Add location…</button><button disabled={busy || !locations.length} onclick={() => run(() => refreshPrivateLibrary(true))}>Rescan library</button></div>
  {/if}
  {#if android}
    <section class="volume">
      <div class="help-row">
        <h3>Phone volume-key bookmarks <span class="badge">UNVERIFIED</span></h3>
        <SettingsHelp title="Phone volume-key bookmarks">
          <p>The phone’s own volume keys may be used with an explicitly enabled accessibility service.
            Locked-screen and screen-off key delivery is unverified on Samsung, Pixel, and Vivo; manufacturers may block it.
            Headset play/pause uses the native media session but is not a substitute for phone-key bookmarking.</p>
          <p>No screen-content access is requested. When disabled, or no book is playing, keys retain normal volume behavior.</p>
          <p>Hold for 700 ms. Confirmation occurs only after a durable bookmark save.</p>
        </SettingsHelp>
      </div>
      {#if capabilities}
        <label class="checkbox"><input type="checkbox" checked={capabilities.volume.settings.enabled} disabled={busy} onchange={event => run(() => setVolume(event.currentTarget.checked))} />Enable long-press volume bookmark mapping</label>
        <label>Phone key<select value={capabilities.volume.settings.key} disabled={busy} onchange={event => run(() => setVolume(capabilities!.volume.settings.enabled, event.currentTarget.value as "up" | "down"))}><option value="up">Volume up</option><option value="down">Volume down</option></select></label>
        <p>Accessibility service: {capabilities.volume.accessibilityConnected ? "connected" : "not enabled in Android settings"}</p>
      {/if}
      <button disabled={busy} onclick={() => run(async () => { await androidReaderRequest("accessibilitySettings"); })}>Open Android accessibility settings</button>
    </section>
  {/if}

  <section class="indexing">
    <div class="help-row">
      <h3>Library search index</h3>
      <SettingsHelp title="Library search index">
        <p>The Library index stays on this device in app data. It is not saved in graphs, graph sync, graph search, or graph AI context.</p>
        <p>Local books are indexed by text. Network links are indexed by title only. Semantic search uses only Grafium’s on-device embedding model; cloud embedding providers make semantic Library search unavailable while keyword search still works.</p>
        <p>Transcribing audio and video uses local Whisper only. It can take time and disk space; rebuilding re-transcribes everything.</p>
        <p>Chat sends Library excerpts to your chosen chat model only when you select the Library context for that question.</p>
      </SettingsHelp>
    </div>
    {#if indexStatus}
      <label class="checkbox"><input type="checkbox" checked={indexStatus.enabled} disabled={indexBusy} onchange={event => runIndex(() => setIndex(event.currentTarget.checked, indexStatus!.transcribeMedia))} />Search inside books and media</label>
      <label class="checkbox"><input type="checkbox" checked={indexStatus.transcribeMedia} disabled={indexBusy || !indexStatus.enabled} onchange={event => runIndex(() => setIndex(indexStatus!.enabled, event.currentTarget.checked))} />Transcribe audio and video</label>
      <p class="path">Indexed {indexStatus.items.indexed} of {indexStatus.items.total} items · {indexStatus.chunks} chunks · {indexStatus.running ? "running" : "idle"}{indexStatus.jobId ? ` · job ${indexStatus.jobId}` : ""}{indexStatus.items.waiting ? ` · ${indexStatus.items.waiting} waiting for a disconnected location` : ""}</p>
      <p>Semantic: {indexStatus.semantic}{indexStatus.semanticReason ? ` · ${indexStatus.semanticReason}` : ""}</p>
      <p>Transcription: {indexStatus.transcription}{indexStatus.transcriptionReason ? ` · ${indexStatus.transcriptionReason}` : ""}</p>
      <p>Last run: {formatLibraryLastRun(indexStatus.lastIndexedAt)}</p>
      {#if indexStatus.errors.length}
        <details open><summary>Recent indexing errors · {indexStatus.errors.length}</summary><ul>{#each indexStatus.errors as item}<li><strong>{item.title}</strong>: {item.message}</li>{/each}</ul></details>
      {/if}
      <div class="actions"><button disabled={indexBusy || indexStatus.running || !indexStatus.enabled} onclick={() => runIndex(async () => { const job = await libraryIndexStart(false); message = `Library indexing started (${job}).`; })}>Index changes now</button>
        {#if confirmRebuild}
          <button bind:this={confirmRebuildButton} disabled={indexBusy || indexStatus.running || !indexStatus.enabled} onclick={() => runIndex(async () => { const job = await libraryIndexStart(true); confirmRebuild = false; message = `Library rebuild started (${job}).`; await tick(); rebuildButton?.focus(); })}>Confirm rebuild</button>
          <button disabled={indexBusy} onclick={async () => { confirmRebuild = false; await tick(); rebuildButton?.focus(); }}>Cancel</button>
        {:else}
          <button bind:this={rebuildButton} disabled={indexBusy || indexStatus.running || !indexStatus.enabled} onclick={async () => { confirmRebuild = true; await tick(); confirmRebuildButton?.focus(); }}>Rebuild index</button>
        {/if}
      </div>
      {#if confirmRebuild}<p class="warning" role="alert">Rebuild re-transcribes all local audio and video and can take a long time.</p>{/if}
    {:else}
      <p role="status">Loading Library index status…</p>
    {/if}
    {#if indexError}<p role="alert" class="error">{indexError}</p>{/if}
  </section>
  <PrivateReaderVoices />
  <details><summary>Private progress backup</summary>
    <div class="help-row">
      <span>Backup and restore</span>
      <SettingsHelp title="Private progress backup">
        <p>Export only app-private records, not original books. Keep the export private. Restore validates the backup before applying it.</p>
        {#if android}
          <p>Android exports and restores use native local-file pickers with the same size limit. No history file is transferred through the WebView bridge.</p>
        {/if}
      </SettingsHelp>
    </div>
    <button disabled={busy} onclick={() => run(exportBackup)}>Export private history</button>
    {#if android}
      <button disabled={busy} onclick={() => run(async () => {
        const result = await androidReaderRequest<{ restoreMode: string; restoreNotice?: string } | null>("restoreState");
        if (!result) return;
        if (result.restoreMode !== "merge") throw new Error("The native history restore did not confirm a successful merge.");
        await refreshPrivateLibrary(); message = result.restoreNotice || "Private history restored.";
      })}>Restore private history…</button>
    {:else}
    <label>Restore a local history file<input type="file" accept=".json,application/json" disabled={busy} onchange={async event => {
      const file = event.currentTarget.files?.[0];
      if (!file) return;
      if (file.size > 64 * 1024 * 1024) { error = "History file exceeds the native 64 MiB export/restore limit."; return; }
      backup = await file.text();
    }} /></label>
    <button disabled={busy || !backup} onclick={() => run(async () => { await readerNative("restore", { data: backup }); await refreshPrivateLibrary(); backup = ""; message = "Private history restored."; })}>Restore selected history</button>
    {/if}
  </details>
  {#if error || $privateLibraryError}<p role="alert" class="error">{error || $privateLibraryError}</p>{/if}
  {#if message}<p role="status">{message}</p>{/if}
</section>

<style>
  .help-row { display: flex; align-items: center; gap: 8px; }
  .private-settings { color: var(--text-primary); font-size: 13px; line-height: 1.6; } h3 { margin: 0; font-size: 15px; }
  p { max-width: 85ch; } .path { border: 1px solid var(--border); padding: 10px; border-radius: 6px; overflow-wrap: anywhere; }
  .actions { display: flex; gap: 10px; flex-wrap: wrap; } button, input { font: inherit; color: var(--text-primary); padding: 8px 12px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg-primary); }
  button { cursor: pointer; } button:disabled { opacity: .5; } button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  details { margin-top: 20px; } summary { cursor: pointer; } label { display: flex; flex-direction: column; gap: 7px; margin: 15px 0; } .error { color: var(--danger, #c44); overflow-wrap: anywhere; }
  .volume, .indexing { border-top: 1px solid var(--border); padding-top: 20px; margin-top: 20px; } .warning { color: var(--danger, #c44); font-weight: 600; } ul { margin-top: 6px; padding-left: 18px; } li { margin: 4px 0; } .badge { font-size: 10px; border: 1px solid var(--border); border-radius: 4px; padding: 3px 5px; margin-left: 8px; }
  .checkbox { flex-direction: row; align-items: center; } select { color: var(--text-primary); background: var(--bg-primary); padding: 8px; border: 1px solid var(--border); border-radius: 6px; }
  .locations { list-style: none; padding: 0; margin: 12px 0; display: flex; flex-direction: column; gap: 10px; }
  .locations li { display: flex; flex-direction: column; gap: 6px; margin: 0; padding: 10px; border: 1px solid var(--border); border-radius: 6px; }
  .location-head { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
  .location-head strong { overflow-wrap: anywhere; }
  .location-state { flex-shrink: 0; font-size: 12px; font-weight: 600; color: var(--accent); }
  .location-state.offline { color: var(--task-todo-fg, var(--text-secondary)); }
  .location-detail { color: var(--text-secondary); font-size: 12px; overflow-wrap: anywhere; }
  .remove-location p { margin: 0 0 8px; }
</style>
