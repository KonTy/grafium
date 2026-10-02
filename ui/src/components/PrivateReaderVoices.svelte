<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { isAndroidReader } from "../lib/privateReaderAndroid";
  import { stopPrivatePlayback, configurePrivateSpeechRate } from "../lib/privateReaderPlayback";
  import { PLAYBACK_RATES, speechPlaybackRate } from "../lib/readerPlaybackPreferences";
  import { voiceCommand, type PrivateVoiceManifest, type PrivateVoiceStatus } from "../lib/privateReaderVoice";
  import { privateVoiceLanguageSuggestion } from "../lib/privateReader";
  import SettingsHelp from "./SettingsHelp.svelte";
  import ReaderVoiceSetup from "./ReaderVoiceSetup.svelte";
  const android = isAndroidReader();
  let voices = $state<PrivateVoiceManifest[]>([]);
  let status = $state<PrivateVoiceStatus | null>(null);
  let voiceId = $state("");
  let language = $state("");
  let busy = $state(false);
  let error = $state("");
  let message = $state("");
  let manifestText = $state("");
  let authorized = $state(false);
  const selected = $derived(voices.find(voice => voice.id === voiceId));
  const languages = $derived([...new Set(voices.map(voice => voice.language))]);
  async function refresh() {
    if (android) {
      status = await voiceCommand<PrivateVoiceStatus>("status");
      voices = status.installed ?? [];
    } else {
      [status, voices] = await Promise.all([
        voiceCommand<PrivateVoiceStatus>("status"), voiceCommand<PrivateVoiceManifest[]>("installed"),
      ]);
    }
    voiceId = status.selection?.voice_id ?? "";
    language = status.selection?.language ?? "";
  }
  async function run(action: () => Promise<void>) {
    busy = true; error = ""; message = "";
    try { await action(); } catch (cause) { error = String(cause); }
    finally { busy = false; }
  }
  onMount(() => { void run(refresh); });
</script>

<section class="voices" aria-label="Offline reader voices" data-help-context="reader">
  <div class="help-row">
    <h3>Offline read-aloud voices</h3>
    <SettingsHelp title="Offline read-aloud voices">
      <p>Choose an installed voice and its language. No browser speech, system-default engine, or cloud fallback is used.</p>
      <p>Languages are taken from installed model metadata, not a fixed list. Changing the language requires a matching voice.</p>
      {#if android}
        <p>Android uses the embedded offline engine and an installed compatible voice. The complete verified narration queue is transferred to the native service before playback; no WebView speech loop or system speech fallback is used.</p>
      {:else}
        <p>Piper must already be installed locally; Grafium does not install runtimes silently.
          Use a dedicated virtual environment created with system Python under /usr; select its bin/piper file.
          The network-denied worker cannot access the rest of your home directory.</p>
      {/if}
    </SettingsHelp>
  </div>
    <div class="help-row">
      <span>Get offline voices</span>
      <SettingsHelp title="Get offline voices"><ReaderVoiceSetup {android} /></SettingsHelp>
    </div>
    {#if status?.reason}<p class="notice">{status.reason}</p>{/if}
    {#if status?.installationErrors?.length}
      <div class="error" role="alert"><strong>Voice packages need attention</strong><ul>
        {#each status.installationErrors as failure}<li><strong>{failure.id}</strong>: {failure.error}</li>{/each}
      </ul></div>
    {/if}
    {#if !android}
    {#if status?.runtime_executable}<p class="metadata">Local Piper: {status.runtime_executable}</p>{/if}
    <button disabled={busy} onclick={() => run(async () => {
      const path = await open({ multiple: false, title: "Select bin/piper in a dedicated Python virtual environment" });
      if (typeof path !== "string") return;
      await stopPrivatePlayback();
      await voiceCommand("configure_runtime", { executablePath: path });
      await refresh(); message = "Local Piper environment saved. Only that environment, system libraries and the selected voice are exposed to the offline worker.";
    })}>Choose local Piper environment…</button>
    {/if}
    {#if $privateVoiceLanguageSuggestion}
      <div class="notice">
        <p>“{$privateVoiceLanguageSuggestion.title}” suggests <strong>{$privateVoiceLanguageSuggestion.language}</strong> from its EPUB metadata.</p>
        <div class="help-row setting-row">
          <button disabled={busy} onclick={() => {
            language = $privateVoiceLanguageSuggestion!.language;
            message = "Suggested language filled in. Choose a matching installed voice, then save explicitly.";
          }}>Use book language suggestion</button>
          <SettingsHelp title="Book language suggestion">
            <p>This only fills the language field; it does not select a voice or download a model.</p>
          </SettingsHelp>
        </div>
      </div>
    {/if}
    <div class="fields">
      <label>Read-aloud speed<select aria-label="Read-aloud speed" value={$speechPlaybackRate} disabled={busy}
        onchange={event => {
          const rate = Number(event.currentTarget.value);
          event.currentTarget.value = String($speechPlaybackRate);
          void run(async () => { await configurePrivateSpeechRate(rate); message = "Read-aloud speed saved. Voice and passage position are unchanged."; });
        }}>
        {#if !PLAYBACK_RATES.includes($speechPlaybackRate)}<option value={$speechPlaybackRate}>{$speechPlaybackRate}×</option>{/if}
        {#each PLAYBACK_RATES as rate}<option value={rate}>{rate}×</option>{/each}
      </select></label>
      <label>Installed voice<select bind:value={voiceId} disabled={busy} onchange={event => { language = voices.find(voice => voice.id === event.currentTarget.value)?.language ?? ""; }}>
        <option value="" disabled>{voices.length ? "Select a voice…" : "No installed voices"}</option>
        {#each voices as voice}<option value={voice.id}>{voice.name} · {voice.language}</option>{/each}
      </select></label>
      <label>Language<input list="offline-voice-languages" bind:value={language} disabled={busy} placeholder="BCP-47 tag, for example fr-CA" /></label>
      <datalist id="offline-voice-languages">{#each languages as value}<option {value}></option>{/each}</datalist>
    </div>
    {#if selected}<p class="metadata">{selected.runtime} · {selected.sample_rate.toLocaleString()} Hz · License: {selected.license}<br />{selected.license_url}</p>{/if}
    <div class="actions">
      <button disabled={busy || !voiceId || !language.trim()} onclick={() => run(async () => {
        await stopPrivatePlayback();
        await voiceCommand("select", { voiceId, language: language.trim() });
        await refresh(); message = "Voice and language saved; old narration was stopped.";
      })}>Save voice and language</button>
      <button disabled={busy} onclick={() => run(async () => {
        if (android) {
          const result = await voiceCommand("import");
          if (result === null) return;
        } else {
          const path = await open({ multiple: false, title: "Import manifest with adjacent offline voice files", filters: [{ name: "Voice manifest", extensions: ["json"] }] });
          if (typeof path !== "string") return;
          await voiceCommand("import", { manifestPath: path });
        }
        await refresh(); message = "Voice files imported; hashes, sizes, license metadata and format checked.";
      })}>Import offline model…</button>
      <button disabled={busy} onclick={() => run(refresh)}>Refresh voices</button>
    </div>
    <details>
      <summary>Advanced: download from a manifest</summary>
      <div class="field-group">
      <div class="help-row">
        <label for="private-voice-manifest">Manifest JSON</label>
        <SettingsHelp title="Voice model download">
          <p>Paste a trusted manifest with direct HTTPS artifact URLs, byte sizes, SHA-256 hashes, runtime and license metadata.
            Downloads do not include book data. Redirects, URL credentials and query strings are not accepted.</p>
          <p>A model's .onnx.json is a Piper configuration, not a Grafium download manifest.
            Hugging Face and GitHub model downloads usually redirect; use Get offline voices and local import for those.
            The preparation command creates an offline-import manifest, not a download manifest.</p>
        </SettingsHelp>
      </div>
      <textarea id="private-voice-manifest" rows="8" maxlength="524288" bind:value={manifestText} spellcheck="false" placeholder="Paste a verified voice manifest"></textarea>
      </div>
      <p>Hashes verify the supplied files, not the publisher’s identity or your right to use a voice.
        Review its model card and license yourself.</p>
      <label class="consent"><input type="checkbox" bind:checked={authorized} />I reviewed this voice’s source and license and authorize downloading its files.</label>
      <button disabled={busy || !authorized || !manifestText.trim()} onclick={() => run(async () => {
        const manifest = JSON.parse(manifestText) as PrivateVoiceManifest;
        await voiceCommand("download", { manifest, userAuthorized: true });
        await refresh(); authorized = false; message = "Voice download verified and stored privately.";
      })}>{busy ? "Working…" : "Download and verify"}</button>
    </details>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if message}<p role="status">{message}</p>{/if}
</section>

<style>
  .help-row { display: flex; align-items: center; gap: 8px; }
  #private-voice-manifest { display: block; box-sizing: border-box; width: 100%; }
  .voices { border-top: 1px solid var(--border); margin-top: 22px; padding-top: 20px; }
  h3 { font-size: 15px; margin: 0; } p { font-size: 12px; color: var(--text-muted); line-height: 1.6; max-width: 85ch; }
  .fields, .actions { display: flex; gap: 12px; flex-wrap: wrap; } .fields label { flex: 1; min-width: 170px; }
  label { display: flex; flex-direction: column; gap: 6px; margin: 10px 0; font-size: 12px; }
  input, select, textarea, button { font: inherit; color: var(--text-primary); background: var(--bg-primary); border: 1px solid var(--border); border-radius: 6px; padding: 8px 10px; }
  button { cursor: pointer; } button:disabled { opacity: .5; } :is(input,select,textarea,button):focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  textarea { resize: vertical; } .consent { flex-direction: row; align-items: center; } details { margin-top: 16px; } summary { cursor: pointer; }
  .metadata, .error { overflow-wrap: anywhere; } .notice { padding: 10px; border: 1px solid var(--border); border-radius: 6px; }
  .error { color: var(--danger, #c44); }
</style>
