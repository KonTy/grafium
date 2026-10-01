<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { privateLibrary, privateLibraryError, readerNative, refreshPrivateLibrary } from "../lib/privateReader";
  import { androidReaderRequest, isAndroidReader, type ReaderVolumeCapabilities } from "../lib/privateReaderAndroid";
  import PrivateReaderVoices from "./PrivateReaderVoices.svelte";
  let busy = $state(false);
  let error = $state("");
  let message = $state("");
  let backup = $state("");
  const android = isAndroidReader();
  let capabilities = $state<ReaderVolumeCapabilities | null>(null);
  async function refreshCapabilities() { capabilities = await androidReaderRequest<ReaderVolumeCapabilities>("capabilities"); }
  async function setVolume(enabled: boolean, key = capabilities?.volume.settings.key ?? "up") {
    await androidReaderRequest("volumeSettings", { enabled, key, gesture: "longPress" });
    await refreshCapabilities();
  }
  async function run(action: () => Promise<void>) {
    busy = true; error = ""; message = "";
    try { await action(); } catch (cause) { error = String(cause); }
    finally { busy = false; }
  }
  async function choose() {
    if (android) {
      await androidReaderRequest("pickLocation");
      await refreshPrivateLibrary();
      return;
    }
    const path = await open({ directory: true, multiple: false, title: "Choose a local private library folder" });
    if (typeof path !== "string") return;
    await readerNative("set_library", { path });
    await refreshPrivateLibrary(true);
    message = "Library location saved. Originals have not been copied into your graph.";
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
    if (!android) return;
    void refreshCapabilities().catch(cause => { error = String(cause); });
    const changed = () => { void refreshCapabilities().catch(cause => { error = String(cause); }); };
    window.addEventListener("focus", changed);
    return () => window.removeEventListener("focus", changed);
  });
</script>

<section data-help-context="reader" class="private-settings">
  <h3>Library location</h3>
  <p>Discover EPUBs and audiobooks in an external local folder. An audiobook folder may contain nested disc folders; each loose MP3 in the library root is a separate book.</p>
  <p class="path">{$privateLibrary.libraryPath || "No private library selected"}</p>
  <div class="actions"><button disabled={busy} onclick={() => run(choose)}>Choose local folder…</button><button disabled={busy || !$privateLibrary.libraryPath} onclick={() => run(() => refreshPrivateLibrary(true))}>Rescan library</button></div>
  <p class="privacy">Device-local books, listening progress, and automatic bookmarks never enter graph sync or AI.
    Intentionally written journal [[Book title]] notes are ordinary shared graph content.
    Do not choose a cloud-backed folder if you need originals to remain only on this device; external backup and whole-device policies are outside Grafium’s control.</p>
  {#if android}
    <section class="volume">
      <h3>Phone volume-key bookmarks <span class="badge">UNVERIFIED</span></h3>
      <p>The phone’s own volume keys may be used with an explicitly enabled accessibility service.
        Locked-screen and screen-off key delivery is unverified on Samsung, Pixel, and Vivo; manufacturers may block it.
        Headset play/pause uses the native media session but is not a substitute for phone-key bookmarking.</p>
      <p>No screen-content access is requested. When disabled, or no book is playing, keys retain normal volume behavior.</p>
      {#if capabilities}
        <label class="checkbox"><input type="checkbox" checked={capabilities.volume.settings.enabled} disabled={busy} onchange={event => run(() => setVolume(event.currentTarget.checked))} />Enable long-press volume bookmark mapping</label>
        <label>Phone key<select value={capabilities.volume.settings.key} disabled={busy} onchange={event => run(() => setVolume(capabilities!.volume.settings.enabled, event.currentTarget.value as "up" | "down"))}><option value="up">Volume up</option><option value="down">Volume down</option></select></label>
        <p>Accessibility service: {capabilities.volume.accessibilityConnected ? "connected" : "not enabled in Android settings"} · hold for 700 ms. Confirmation occurs only after a durable bookmark save.</p>
      {/if}
      <button disabled={busy} onclick={() => run(async () => { await androidReaderRequest("accessibilitySettings"); })}>Open Android accessibility settings</button>
    </section>
  {/if}
  <PrivateReaderVoices />
  <details><summary>Private progress backup</summary>
    <p>Export only app-private records, not original books. Keep the export private. Restore validates the backup before applying it.</p>
    <button disabled={busy} onclick={() => run(exportBackup)}>Export private history</button>
    {#if android}
      <p>Android exports and restores use native local-file pickers with the same size limit. No history file is transferred through the WebView bridge.</p>
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
  .private-settings { color: var(--text-primary); font-size: 13px; line-height: 1.6; } h3 { margin: 0; font-size: 15px; }
  p { max-width: 85ch; } .privacy { color: var(--text-muted); } .path { border: 1px solid var(--border); padding: 10px; border-radius: 6px; overflow-wrap: anywhere; }
  .actions { display: flex; gap: 10px; flex-wrap: wrap; } button, input { font: inherit; color: var(--text-primary); padding: 8px 12px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg-primary); }
  button { cursor: pointer; } button:disabled { opacity: .5; } button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  details { margin-top: 20px; } summary { cursor: pointer; } label { display: flex; flex-direction: column; gap: 7px; margin: 15px 0; } .error { color: var(--danger, #c44); overflow-wrap: anywhere; }
  .volume { border-top: 1px solid var(--border); padding-top: 20px; margin-top: 20px; } .badge { font-size: 10px; border: 1px solid var(--border); border-radius: 4px; padding: 3px 5px; margin-left: 8px; }
  .checkbox { flex-direction: row; align-items: center; } select { color: var(--text-primary); background: var(--bg-primary); padding: 8px; border: 1px solid var(--border); border-radius: 6px; }
</style>
