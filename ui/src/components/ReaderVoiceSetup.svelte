<script lang="ts">
  import { open as openExternal } from "@tauri-apps/plugin-shell";
  import { PIPER_SETUP_COMMAND, VOICE_SETUP_URLS as urls, voicePackageCommand } from "../lib/voiceSetup";
  let { android }: { android: boolean } = $props();
  let error = $state("");
  let message = $state("");
  let platform = $state<"linux" | "android" | null>(null);
  const forAndroid = $derived(platform === null ? android : platform === "android");
  const preparation = $derived(voicePackageCommand(forAndroid));
  async function visit(url: string) {
    error = ""; message = "";
    try { await openExternal(url); }
    catch (cause) { error = `Could not open the browser: ${String(cause)}. Copy the displayed link instead.`; }
  }
  async function copy(text: string) {
    error = ""; message = "";
    try { await navigator.clipboard.writeText(text); message = "Command copied. Review it before running it."; }
    catch (cause) { error = `Could not copy: ${String(cause)}. Select and copy the command below instead.`; }
  }
</script>

<div class="voice-setup">
  <label>Guide platform
    <select value={forAndroid ? "android" : "linux"} onchange={event => {
      const value = event.currentTarget.value;
      if (value === "linux" || value === "android") { platform = value; error = ""; message = ""; }
    }}><option value="linux">Linux</option><option value="android">Android (prepare on a computer)</option></select>
  </label>
  <p>This changes the instructions only, not the installed engine. You can prepare an Android package on Linux.</p>
  <p><strong>Start here: LJ Speech, US English, high quality.</strong>
    This is one example voice, not a restriction on languages. A speech engine and a voice model are separate things.</p>
  <p>These buttons open your browser; they do not install anything or send book text.
    There is no built-in one-click voice catalog yet. Common model hosts redirect downloads,
    so use the browser and then import locally instead of pasting their links into the advanced downloader.</p>
  <ol>
    <li><strong>Review the voice license.</strong>
      The model author declares this model public domain; the dataset is public domain in the US.
      Other jurisdictions and bundled phonemizer data have their own terms. Keep the original notices.
      <p><a href={urls.modelLicense} onclick={event => { event.preventDefault(); void visit(urls.modelLicense); }}>Model author's license</a>
        · <a href={urls.datasetLicense} onclick={event => { event.preventDefault(); void visit(urls.datasetLicense); }}>Dataset license</a></p>
    </li>
    {#if !forAndroid}
      <li><strong>Install Piper once on Linux.</strong>
        You need system Python 3 with virtual-environment support and Bubblewrap (<code>bwrap</code>).
        Run these commands yourself; they download software, not books. Grafium does not run them or request administrator access.
        <pre><code>{PIPER_SETUP_COMMAND}</code></pre>
        <button type="button" onclick={() => copy(PIPER_SETUP_COMMAND)}>Copy Piper setup command</button>
        <p>Then choose <strong>Choose local Piper environment…</strong> and select
          <code>~/.local/share/grafium-piper/bin/piper</code>.
          If Python's venv module or Bubblewrap is missing, install it through your OS package manager deliberately.</p>
      </li>
      <li><strong>Download the voice into one new folder.</strong>
        Save both files without renaming them:
        <code>en_US-ljspeech-high.onnx</code> (about 114 MB) and
        <code>en_US-ljspeech-high.onnx.json</code>.
        <p><a href={urls.linuxModel} onclick={event => { event.preventDefault(); void visit(urls.linuxModel); }}>Download Linux model in browser</a>
          · <a href={urls.linuxConfig} onclick={event => { event.preventDefault(); void visit(urls.linuxConfig); }}>Download matching configuration</a>
          · <a href={urls.linuxFiles} onclick={event => { event.preventDefault(); void visit(urls.linuxFiles); }}>All files</a></p>
      </li>
    {:else}
      <li><strong>Use the Android-converted voice, not the Linux model.</strong>
        The engine is already embedded in Grafium. Download
        <code>vits-piper-en_US-ljspeech-high.tar.bz2</code> (about 116 MB) on a computer and extract it.
        Keep <code>en_US-ljspeech-high.onnx</code>, <code>tokens.txt</code>,
        and the entire <code>espeak-ng-data/</code> folder with its subdirectories.
        <p><a href={urls.androidModel} onclick={event => { event.preventDefault(); void visit(urls.androidModel); }}>Download Android voice bundle in browser</a>
          · <a href={urls.androidDocs} onclick={event => { event.preventDefault(); void visit(urls.androidDocs); }}>Official model instructions</a></p>
        <p>Do not download an APK, import the archive itself, or rename an unconverted Piper model.
          Grafium needs the extracted, Sherpa-converted package. Preparation below uses Python 3 on a computer;
          a POSIX terminal such as Linux, macOS, or WSL is required for the copied command.</p>
      </li>
    {/if}
    <li><strong>Save the model card in that same folder.</strong>
      <a href={urls.modelCard} onclick={event => { event.preventDefault(); void visit(urls.modelCard); }}>Download MODEL_CARD in browser</a>.
      Keep the filename <code>MODEL_CARD</code>, without an added <code>.txt</code> extension.
    </li>
    <li><strong>Prepare Grafium's manifest.</strong>
      Open a terminal <em>inside the folder containing the ONNX file</em>, then run the copied command.
      It calculates file sizes and SHA-256 hashes and creates <code>manifest.json</code>.
      It makes no network requests, does not convert the model, and refuses to replace an existing manifest.
      It leaves the downloaded voice files unchanged.
      <p><button type="button" onclick={() => copy(preparation)}>Copy package preparation command</button></p>
      <details><summary>Review package preparation command</summary><pre><code>{preparation}</code></pre></details>
      <p>This example is specifically for LJ Speech high, <code>en-US</code>, 22,050 Hz.
        For another voice, review its model card and update the command's ID, name, language,
        sample rate, and license fields; do not reuse this voice's license.
        Locally calculated hashes do not prove publisher identity or engine compatibility.</p>
    </li>
    <li><strong>Import and select it.</strong>
      {#if forAndroid}
        Copy the complete prepared folder to local phone storage without changing its structure.
        Choose <strong>Import offline model…</strong> and select that folder.
      {:else}
        Choose <strong>Import offline model…</strong> and select its <code>manifest.json</code>,
        not the ONNX file or the Piper configuration JSON.
      {/if}
      Choose the installed voice and language, then <strong>Save voice and language</strong>.
      Open an EPUB and use <strong>… → Read aloud from start</strong>.
    </li>
  </ol>
  <p><strong>If import fails:</strong> restore missing files, unchanged filenames and directory structure;
    use the correct platform package. Grafium validates the actual model as well as the manifest.
    Do not disable validation or switch to a cloud engine to bypass an error.</p>
  <p>Once configured, synthesis uses local files with no cloud-TTS fallback.
    Opening a provider link, listening to its web samples, or downloading models does contact that provider.</p>
  <p>Other voices:
    <a href={urls.catalog} onclick={event => { event.preventDefault(); void visit(urls.catalog); }}>Piper voice samples</a>
    · <a href={urls.piperDocs} onclick={event => { event.preventDefault(); void visit(urls.piperDocs); }}>Piper download documentation</a>
    · <a href={urls.androidCatalog} onclick={event => { event.preventDefault(); void visit(urls.androidCatalog); }}>Sherpa model catalog</a>.
    Grafium's Android importer currently accepts converted Piper/VITS packages, not every engine listed in that catalog.</p>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if message}<p role="status">{message}</p>{/if}
</div>

<style>
  li { margin: 14px 0; }
  ol { padding-left: 24px; }
  a { color: var(--accent); }
  pre { max-height: 240px; overflow: auto; padding: 10px; background: var(--bg-primary); border: 1px solid var(--border); border-radius: 6px; white-space: pre-wrap; overflow-wrap: anywhere; }
  code { font-size: 12px; }
  button { font: inherit; color: var(--text-primary); background: var(--bg-primary); border: 1px solid var(--border); border-radius: 6px; padding: 8px 10px; cursor: pointer; }
  label { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
  select { max-width: 100%; font: inherit; color: var(--text-primary); background: var(--bg-primary); border: 1px solid var(--border); border-radius: 6px; padding: 6px; }
  :is(button,a):focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  summary { cursor: pointer; }
  [role="alert"] { color: var(--danger, #c44); }
</style>
