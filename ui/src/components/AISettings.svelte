<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    aiGetConfig,
    aiSetConfig,
    aiHealthCheck,
    aiIndexAllPages,
    aiCreateDefaultSchemas,
    type AiConfig,
    type AiConfigPayload,
    type HealthStatus,
  } from "../lib/knowledge";
  import { mediaGetConfig, mediaSetConfig, listLocalModels, detectGpuInfo, type MediaConfigPayload, type LocalModelInfo, type GpuInfo } from "../lib/api";

  let config = $state<AiConfig | null>(null);
  let health = $state<HealthStatus | null>(null);
  let isLoading = $state(false);
  let isSaving = $state(false);
  let isIndexing = $state(false);
  let indexCount = $state<number | null>(null);
  let message = $state("");
  let messageType = $state<"success" | "error">("success");
  // Detected primary GPU (name + total VRAM). Populated on first open of
  // the AI Settings tab; used by the model-picker to mark models that
  // won't fit on the GPU and will therefore spill to CPU/RAM and run
  // painfully slowly. `null` while we're still detecting; the "empty"
  // GpuInfo (source === "none") once detection came back with nothing.
  let gpuInfo = $state<GpuInfo | null>(null);

  // Form state
  let enabled = $state(false);
  let mode = $state("local");
  let localProvider = $state("openai_compatible");
  let localBaseUrl = $state("http://localhost:8000/v1");
  let localApiKey = $state("");
  let localModelPath = $state("");
  let localEmbeddingModelPath = $state("");
  let localModelsDir = $state("");
  let localContextSize = $state<string | number>("");
  let localGpuLayers = $state<string | number>("");
  // "auto" (built-in heuristic — mmap OFF for CPU-only, ON for GPU),
  // "off" (force mmap disabled — safer on unreliable storage; slower to
  // load), or "on" (force mmap enabled). Kept as a plain string in $state
  // for easy binding to a <select>; converted to Option<bool> in the
  // save payload. Sticks to whatever the user set even after a
  // SIGBUS-triggered auto-fallback in the process wrapper.
  let localUseMmap = $state<"auto" | "off" | "on">("auto");
  let llmModel = $state("llama3.2");
  let embeddingModel = $state("nomic-embed-text");
  let cloudProvider = $state("openai");
  let cloudBaseUrl = $state("");
  let cloudLlmModel = $state("gpt-4o");
  let cloudApiKey = $state("");
  let cloudEmbeddingProvider = $state("openai");
  let cloudEmbeddingBaseUrl = $state("");
  let cloudEmbeddingApiKey = $state("");
  let cloudEmbeddingModel = $state("text-embedding-3-small");

  // Whisper transcription (video/audio import fallback) — independent of
  // the chat/search config above, since it's used by "Import Video" rather
  // than the assistant, but lives in this same dialog since that's where
  // users look for anything AI/model-related.
  let mediaEnabled = $state(true);
  let mediaModelPath = $state("");
  let mediaModelsDir = $state("");
  let mediaLanguage = $state("");
  let isSavingMedia = $state(false);

  function normalizeProvider(p?: string): string {
    if (!p) return "openai_compatible";
    if (p === "openaicompatible" || p === "vllm") return "openai_compatible";
    return p;
  }

  // Parses a text-input value for the GPU-layers/context-size fields into
  // a positive integer, or `undefined` for blank/invalid input (falls back
  // to the backend's own auto-detect default rather than sending 0/NaN).
  // Accepts `string` (its declared $state type) or `number` — Svelte's
  // bind:value on <input type="number"> silently coerces the bound value
  // to an actual JS number once the user types something (only staying a
  // string while empty), so this must handle both or a plain `.trim()`
  // call throws a TypeError and silently breaks the whole Save button.
  function parsePositiveInt(value: string | number): number | undefined {
    if (typeof value === "number") {
      return Number.isFinite(value) && value > 0 ? Math.trunc(value) : undefined;
    }
    const trimmed = value.trim();
    if (!trimmed) return undefined;
    const n = Number.parseInt(trimmed, 10);
    return Number.isFinite(n) && n > 0 ? n : undefined;
  }

  // Sensible default Base URL per local provider, so switching providers
  // doesn't leave a stale/wrong-looking URL behind (e.g. an OpenAI-compatible
  // `/v1` URL left in place after switching to Ollama). Only auto-fills when
  // the field still holds one of these known defaults (or is empty) — never
  // clobbers a URL the user actually typed in themselves.
  const LOCAL_BASE_URL_DEFAULTS: Record<string, string> = {
    openai_compatible: "http://localhost:8000/v1",
    ollama: "http://localhost:11434",
  };

  function selectLocalProvider(provider: string) {
    const knownDefaults = Object.values(LOCAL_BASE_URL_DEFAULTS);
    if (!localBaseUrl.trim() || knownDefaults.includes(localBaseUrl)) {
      localBaseUrl = LOCAL_BASE_URL_DEFAULTS[provider] || "";
    }
    localProvider = provider;
  }

  // The embedded (llama.cpp) provider only implements chat/completion —
  // there's no local embedding backend behind it yet (see
  // `KnowledgeEngine::initialize_providers` in core/src/knowledge/engine.rs,
  // which leaves `self.embedder` unset for `ProviderType::HuggingFace`).
  // Semantic search / "Index All Pages" needs an embedder, so the Embedding
  // Model field (and the LLM Model field, which Embedded also ignores in
  // favor of its own GGUF file) only make sense for the endpoint-based
  // providers.
  // Locally-downloaded model files, scanned from the configured (or
  // default) Models Directory so Settings can offer a dropdown instead of
  // asking the user to type an exact file name.
  let localModelOptions = $state<LocalModelInfo[]>([]);
  let localEmbeddingModelOptions = $state<LocalModelInfo[]>([]);
  let mediaModelOptions = $state<LocalModelInfo[]>([]);

  // Drives the description pane next to the chat-model listbox below —
  // whichever file is currently selected (by name), or `undefined` while
  // "Auto-detect" is selected / nothing's loaded yet.
  let selectedLocalModel = $derived(
    localModelOptions.find((m) => m.file_name === localModelPath),
  );

  async function refreshLocalModelOptions() {
    try {
      const all = await listLocalModels(localModelsDir || undefined);
      localModelOptions = all.filter((m) => m.kind === "llm");
      localEmbeddingModelOptions = all.filter((m) => m.kind === "embedding");
    } catch {
      localModelOptions = [];
      localEmbeddingModelOptions = [];
    }
  }

  async function refreshMediaModelOptions() {
    try {
      const all = await listLocalModels(mediaModelsDir || undefined);
      mediaModelOptions = all.filter((m) => m.kind === "whisper");
    } catch {
      mediaModelOptions = [];
    }
  }

  // One-shot GPU detection. Shelling out to nvidia-smi/vulkaninfo is
  // cheap (~50-300 ms) and we only care about it on this settings pane,
  // so we do it lazily on first mount rather than at app boot.
  async function refreshGpuInfo() {
    try {
      gpuInfo = await detectGpuInfo();
    } catch {
      gpuInfo = { name: null, total_vram_bytes: null, available_vram_bytes: null, source: "none" };
    }
  }

  async function browseLocalModelsDir() {
    const selected = await open({ directory: true, multiple: false, title: "Choose Models Directory" });
    if (selected && typeof selected === "string") {
      localModelsDir = selected;
    }
  }

  async function browseMediaModelsDir() {
    const selected = await open({ directory: true, multiple: false, title: "Choose Models Directory" });
    if (selected && typeof selected === "string") {
      mediaModelsDir = selected;
    }
  }

  function fmtModelSize(bytes: number): string {
    if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
    if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(0)} MB`;
    return `${bytes} B`;
  }

  // Icon + tooltip for a model based on its VRAM-fit classification.
  // We deliberately reuse the same ⚠️ glyph as the "unstable architecture"
  // warning but with a *different* tooltip and CSS class so the two
  // reasons a model might be flagged don't get visually conflated — a
  // model can be both (Fable-Fusion-711-IQ2_M being a real example).
  function vramFitIcon(m: LocalModelInfo): { glyph: string; title: string; cls: string } | null {
    if (m.vram_fit === "wont-fit") {
      const needed = m.vram_needed_bytes ? ` (needs ~${fmtModelSize(m.vram_needed_bytes)} VRAM)` : "";
      return {
        glyph: "⚠️",
        title: `Too large for your GPU${needed} — will fall back to CPU / stream weights from RAM. Expect very slow generation (< 2 tok/s).`,
        cls: "model-vram-warn",
      };
    }
    if (m.vram_fit === "tight") {
      const needed = m.vram_needed_bytes ? ` (needs ~${fmtModelSize(m.vram_needed_bytes)} VRAM)` : "";
      return {
        glyph: "⚡",
        title: `Tight fit on your GPU${needed} — should run on-GPU but with little headroom for other apps. May slow down under memory pressure.`,
        cls: "model-vram-tight",
      };
    }
    return null;
  }

  // Sentence describing what the fit classification means, shown in the
  // right-hand description pane below the model's file description.
  function vramFitExplanation(m: LocalModelInfo): string | null {
    if (!gpuInfo || !gpuInfo.total_vram_bytes) return null;
    const total = fmtModelSize(gpuInfo.total_vram_bytes);
    const needed = m.vram_needed_bytes ? fmtModelSize(m.vram_needed_bytes) : "unknown";
    if (m.vram_fit === "wont-fit") {
      return `⚠️ This model needs ~${needed} of VRAM but your GPU only has ${total}. It will fall back to CPU or stream weights from system RAM, which is 10-50× slower than GPU generation. Expect summaries to take many minutes and to feel unresponsive. Consider a smaller/more quantized model.`;
    }
    if (m.vram_fit === "tight") {
      return `⚡ This model needs ~${needed}, which is close to your ${total} of VRAM. It should run on-GPU but with almost no headroom — if you open other GPU-accelerated apps (Chrome, video call, games) generation may spill to CPU mid-run and slow down.`;
    }
    if (m.vram_fit === "fits") {
      return `✓ Fits comfortably on your GPU (~${needed} of ${total} VRAM).`;
    }
    return null;
  }

  // Detect the GPU once on mount so the fit-warning icons on the model
  // options have data to key off. The list re-render itself is driven
  // by the model options + gpuInfo state; this just kicks the fetch.
  $effect(() => {
    if (gpuInfo === null) {
      void refreshGpuInfo();
    }
  });

  // Re-scan whenever the directory changes — manual edits, a "Browse..."
  // pick, or the initial value loaded from saved config all flow through
  // this same reactive read.
  $effect(() => {
    localModelsDir;
    void refreshLocalModelOptions();
  });
  $effect(() => {
    mediaModelsDir;
    void refreshMediaModelOptions();
  });

  // Load config on mount
  $effect(() => {
    loadConfig();
    loadMediaConfig();
  });

  async function loadConfig() {
    isLoading = true;
    try {
      config = await aiGetConfig();
      if (config) {
        enabled = config.enabled;
        // Hybrid mode is no longer offered in this UI (it mixed too many
        // concepts — base URLs, LLM models, and embedding models across two
        // providers at once). Gracefully fall back to Local for anyone with
        // an old config that still says "hybrid" rather than getting stuck
        // on a mode with no matching button.
        mode = config.mode === "hybrid" ? "local" : config.mode || "local";
        if (config.local) {
          localProvider = normalizeProvider(config.local.provider);
          localBaseUrl = config.local.base_url || "http://localhost:8000/v1";
          localApiKey = config.local.api_key || "";
          localModelPath = config.local.local_llm?.model || "";
          localContextSize = config.local.local_llm?.context_size?.toString() || "";
          localGpuLayers = config.local.local_llm?.gpu_layers?.toString() || "";
          const mmapCfg = config.local.local_llm?.use_mmap;
          localUseMmap = mmapCfg === true ? "on" : mmapCfg === false ? "off" : "auto";
          localEmbeddingModelPath = config.local.local_embedding?.model || "";
          localModelsDir = config.local.models_dir || "";
          llmModel = config.local.llm_model || "llama3.2";
          embeddingModel = config.local.embedding_model || "nomic-embed-text";
        }
        if (config.cloud) {
          cloudProvider = normalizeProvider(config.cloud.llm_provider);
          cloudBaseUrl = config.cloud.llm_base_url || "";
          cloudLlmModel = config.cloud.llm_model || "gpt-4o";
          cloudApiKey = config.cloud.llm_api_key || "";
          cloudEmbeddingProvider = normalizeProvider(config.cloud.embedding_provider);
          cloudEmbeddingBaseUrl = config.cloud.embedding_base_url || "";
          cloudEmbeddingApiKey = config.cloud.embedding_api_key || "";
          cloudEmbeddingModel = config.cloud.embedding_model || "text-embedding-3-small";
        }
      }
      health = await aiHealthCheck();
    } catch (e: any) {
      showMessage("Failed to load config: " + e, "error");
    } finally {
      isLoading = false;
    }
  }

  async function saveConfig() {
    isSaving = true;
    try {
      const payload: AiConfigPayload = {
        enabled,
        mode,
        local_provider: localProvider,
        local_base_url: localBaseUrl,
        local_api_key: localApiKey || undefined,
        local_model_path: localModelPath || undefined,
        local_context_size: parsePositiveInt(localContextSize),
        local_gpu_layers: parsePositiveInt(localGpuLayers),
        local_use_mmap:
          localUseMmap === "off" ? false : localUseMmap === "on" ? true : null,
        local_embedding_model_path: localEmbeddingModelPath || undefined,
        local_models_dir: localModelsDir || undefined,
        llm_model: llmModel,
        embedding_model: embeddingModel,
        cloud_provider: cloudProvider,
        cloud_base_url: cloudBaseUrl || undefined,
        cloud_llm_model: cloudLlmModel,
        cloud_api_key: cloudApiKey || undefined,
        cloud_embedding_provider: cloudEmbeddingProvider,
        cloud_embedding_base_url: cloudEmbeddingBaseUrl || undefined,
        cloud_embedding_api_key: cloudEmbeddingApiKey || undefined,
        cloud_embedding_model: cloudEmbeddingModel,
      };
      await aiSetConfig(payload);
      health = await aiHealthCheck();
      showMessage("Configuration saved!", "success");
    } catch (e: any) {
      showMessage("Failed to save: " + e, "error");
    } finally {
      isSaving = false;
    }
  }

  async function indexAll() {
    isIndexing = true;
    indexCount = null;
    try {
      indexCount = await aiIndexAllPages();
      showMessage(`Indexed ${indexCount} chunks`, "success");
      health = await aiHealthCheck();
    } catch (e: any) {
      showMessage("Indexing failed: " + e, "error");
    } finally {
      isIndexing = false;
    }
  }

  async function loadMediaConfig() {
    try {
      const mediaConfig = await mediaGetConfig();
      mediaEnabled = mediaConfig.enabled;
      mediaModelPath = mediaConfig.whisper?.model || "";
      mediaModelsDir = mediaConfig.models_dir || "";
      mediaLanguage = mediaConfig.whisper?.language || "";
    } catch (e: any) {
      showMessage("Failed to load transcription config: " + e, "error");
    }
  }

  async function saveMediaConfig() {
    isSavingMedia = true;
    try {
      const payload: MediaConfigPayload = {
        enabled: mediaEnabled,
        models_dir: mediaModelsDir || undefined,
        whisper_model_path: mediaModelPath || undefined,
        language: mediaLanguage || undefined,
      };
      await mediaSetConfig(payload);
      showMessage("Transcription settings saved!", "success");
    } catch (e: any) {
      showMessage("Failed to save: " + e, "error");
    } finally {
      isSavingMedia = false;
    }
  }

  async function createSchemas() {
    try {
      await aiCreateDefaultSchemas();
      showMessage("Default schemas created!", "success");
    } catch (e: any) {
      showMessage("Failed: " + e, "error");
    }
  }

  function showMessage(msg: string, type: "success" | "error") {
    message = msg;
    messageType = type;
    setTimeout(() => (message = ""), 4000);
  }

  const MODE_DESCRIPTIONS: Record<string, string> = {
    local: "Chat and semantic search both run on this machine (or a local-network endpoint). Nothing leaves your computer.",
    cloud: "Chat and semantic search both use a cloud API (OpenAI, Anthropic, or a compatible endpoint). Requires an API key; your notes' content is sent to that provider.",
  };

  const LOCAL_PROVIDER_DESCRIPTIONS: Record<string, string> = {
    openai_compatible: "Connects to a server you run yourself that speaks the OpenAI API (vLLM, llama-server, LM Studio, etc.).",
    ollama: "Connects to a running Ollama server on this machine or your local network.",
    huggingface: "Runs llama.cpp in-process, built into Grafium itself — no server to start or keep running.",
  };
</script>

<div class="ai-settings">
  <h3>AI / Knowledge Engine</h3>

  {#if isLoading}
    <div class="loading">Loading configuration...</div>
  {:else}
    <!-- Status -->
    {#if health}
      <div class="status-bar" class:active={health.enabled && health.llm_available}>
        <span class="status-dot"></span>
        <span>
          {#if !health.enabled}
            Disabled
          {:else if health.llm_available}
            Connected ({health.mode})
          {:else}
            Not connected
          {/if}
        </span>
        {#if health.enabled && health.llm_available && !health.embedder_available}
          <span class="status-vectors warning">no search embedder</span>
        {:else if health.vector_count > 0}
          <span class="status-vectors">{health.vector_count} vectors</span>
        {/if}
      </div>
      {#if health.enabled && !health.llm_available && health.llm_load_error}
        <p class="field-hint warning llm-load-error">{health.llm_load_error}</p>
      {/if}
    {/if}

    <!-- Enable toggle -->
    <label class="toggle-row">
      <input type="checkbox" bind:checked={enabled} />
      <span>Enable AI features</span>
    </label>

    {#if enabled}
      <!-- Mode selection -->
      <div class="field-group">
        <label class="field-label">Mode</label>
        <div class="choice-row">
          <button class="choice-btn" class:active={mode === "local"} onclick={() => (mode = "local")}>Local</button>
          <button class="choice-btn" class:active={mode === "cloud"} onclick={() => (mode = "cloud")}>Cloud</button>
        </div>
        <p class="field-hint">{MODE_DESCRIPTIONS[mode]}</p>
      </div>

      <!-- Local settings -->
      {#if mode === "local"}
        <div class="settings-section">
          <h4>Local Provider</h4>
          <div class="field-group">
            <label class="field-label">Provider</label>
            <div class="choice-row">
              <button class="choice-btn" class:active={localProvider === "openai_compatible"} onclick={() => selectLocalProvider("openai_compatible")}>vLLM / OpenAI-compatible</button>
              <button class="choice-btn" class:active={localProvider === "ollama"} onclick={() => selectLocalProvider("ollama")}>Ollama</button>
              <button class="choice-btn" class:active={localProvider === "huggingface"} onclick={() => selectLocalProvider("huggingface")}>Embedded</button>
            </div>
            <p class="field-hint">{LOCAL_PROVIDER_DESCRIPTIONS[localProvider]}</p>
          </div>

          {#if localProvider === "huggingface"}
            <!-- Embedded (llama.cpp): no server/URL/key involved at all. -->
            <div class="field-group">
              <label class="field-label">Models Directory</label>
              <div class="browse-row">
                <input type="text" bind:value={localModelsDir} class="field-input" placeholder="e.g. ~/Documents/models — shared folder to search for model files" />
                <button type="button" class="browse-btn" onclick={browseLocalModelsDir}>Browse...</button>
                <button type="button" class="browse-btn" onclick={refreshLocalModelOptions} title="Re-scan this folder">Refresh</button>
              </div>
              <p class="field-hint">
                Point this at a folder you already keep local models in (shared with Ollama, LM
                Studio, another app, etc.) so Grafium never duplicates multi-gigabyte model files.
                Leave blank to use Grafium's own managed models folder.
              </p>
            </div>
            <div class="field-group">
              <label class="field-label">Embedded LLM Model File (GGUF)</label>
              {#if gpuInfo && gpuInfo.total_vram_bytes}
                <p class="gpu-detected-banner">
                  Detected GPU: <strong>{gpuInfo.name ?? "unknown card"}</strong> with
                  <strong>{fmtModelSize(gpuInfo.total_vram_bytes)}</strong> VRAM.
                  Models flagged with ⚠️ won't fit and will be very slow.
                </p>
              {:else if gpuInfo && gpuInfo.source === "none"}
                <p class="gpu-detected-banner gpu-detected-banner-unknown">
                  Couldn't detect a GPU (tried nvidia-smi, vulkaninfo, sysfs) — VRAM warnings on
                  the model list are disabled. If you have a dedicated GPU, install
                  <code>vulkaninfo</code> (or NVIDIA's <code>nvidia-smi</code>) to enable them.
                </p>
              {/if}
              {#if localModelOptions.length > 0}
                <div class="model-picker">
                  <div class="model-listbox" role="listbox" aria-label="Chat model file">
                    <button
                      type="button"
                      class="model-option"
                      class:active={localModelPath === ""}
                      onclick={() => (localModelPath = "")}
                    >
                      Auto-detect (only chat GGUF file in folder)
                    </button>
                    {#each localModelOptions as m (m.file_name)}
                      {@const fit = vramFitIcon(m)}
                      <button
                        type="button"
                        class="model-option"
                        class:active={localModelPath === m.file_name}
                        onclick={() => (localModelPath = m.file_name)}
                      >
                        {#if m.unstable_architecture}<span class="model-warn-icon" title="Not supported yet">⚠️</span>{/if}
                        {#if fit}<span class={fit.cls} title={fit.title}>{fit.glyph}</span>{/if}
                        <span class="model-option-name">{m.file_name}</span>
                        <span class="model-option-size">{fmtModelSize(m.size_bytes)}</span>
                      </button>
                    {/each}
                  </div>
                  <div class="model-description">
                    {#if selectedLocalModel}
                      <div class="model-description-title">{selectedLocalModel.file_name}</div>
                      {#if selectedLocalModel.unstable_architecture}
                        <p class="model-description-warning">
                          ⚠️ Not supported yet — this model uses the
                          {" "}<code>{selectedLocalModel.architecture}</code> architecture (Gated
                          Delta Net), which has a known upstream llama.cpp bug that crashes the
                          app during generation. Grafium refuses to load it until llama.cpp fixes
                          this. Check back later — it's an active area of upstream development.
                        </p>
                      {/if}
                      {#if vramFitExplanation(selectedLocalModel)}
                        <p
                          class="model-description-fit"
                          class:model-description-fit-wont={selectedLocalModel.vram_fit === "wont-fit"}
                          class:model-description-fit-tight={selectedLocalModel.vram_fit === "tight"}
                          class:model-description-fit-ok={selectedLocalModel.vram_fit === "fits"}
                        >
                          {vramFitExplanation(selectedLocalModel)}
                        </p>
                      {/if}
                      {#if selectedLocalModel.description}
                        <p class="model-description-text">{selectedLocalModel.description}</p>
                      {/if}
                      <p class="model-description-meta">{fmtModelSize(selectedLocalModel.size_bytes)}</p>
                    {:else}
                      <p class="model-description-placeholder">
                        Select a model on the left to see its details here.
                      </p>
                    {/if}
                  </div>
                </div>
              {:else}
                <p class="field-hint">
                  No chat GGUF files found yet in the Models Directory above. Download one there,
                  then hit Refresh — it'll show up here instead of needing to be typed by hand.
                </p>
              {/if}
            </div>
            <div class="field-group field-group-row">
              <div class="field-subgroup">
                <label class="field-label">GPU Layers</label>
                <div class="input-row">
                  <input
                    type="number"
                    min="0"
                    step="1"
                    bind:value={localGpuLayers}
                    placeholder="Auto (all layers)"
                    class="field-input"
                  />
                  <button
                    type="button"
                    class="browse-btn"
                    disabled={localGpuLayers === "" || localGpuLayers === undefined}
                    onclick={() => { localGpuLayers = ""; }}
                    title="Clear to auto — VRAM-aware default (offload all if it fits, CPU-only otherwise)"
                  >
                    Auto
                  </button>
                </div>
                <p class="field-hint">
                  How many transformer layers to offload to the GPU. Leave blank to offload every
                  layer (fastest if the model fits in VRAM) — Grafium falls back to CPU-only on
                  its own if it doesn't fit. Set a lower number (e.g. 20) to force a mixed
                  CPU/GPU split for large models that don't fully fit in VRAM, which also lowers
                  the system RAM needed for CPU-resident layers.
                </p>
              </div>
              <div class="field-subgroup">
                <label class="field-label">Context Size (tokens)</label>
                <div class="input-row">
                  <input
                    type="number"
                    min="0"
                    step="1"
                    bind:value={localContextSize}
                    placeholder="Auto (model default)"
                    class="field-input"
                  />
                  <button
                    type="button"
                    class="browse-btn"
                    disabled={localContextSize === "" || localContextSize === undefined}
                    onclick={() => { localContextSize = ""; }}
                    title="Clear to auto — use the model's own trained context length"
                  >
                    Auto
                  </button>
                </div>
                <p class="field-hint">
                  Context window size in tokens. Leave blank to use the model's own trained
                  context length. Lowering this reduces memory usage at the cost of how much text
                  the model can consider at once.
                </p>
              </div>
              <div class="field-subgroup">
                <label class="field-label">Memory-map model file</label>
                <select bind:value={localUseMmap} class="field-select">
                  <option value="auto">Auto (recommended)</option>
                  <option value="off">Off — safer on unreliable storage; slower to load</option>
                  <option value="on">On — fastest load</option>
                </select>
                <p class="field-hint">
                  If your worker crashes with <code>signal: 7 (SIGBUS)</code> mid-generation,
                  set this to <strong>Off</strong>. Memory-mapping reads model weights lazily
                  from disk — fast at load time, but a page-in that fails (unreliable disk,
                  network mount, snap/flatpak sandbox, partial GGUF download, or not enough
                  swap) crashes the worker with SIGBUS. Turning it off reads the whole model
                  eagerly into RAM up front instead. Grafium also auto-flips this to Off
                  after the first SIGBUS crash and remembers it until you restart the app;
                  set it here to keep the safer behavior permanent.
                </p>
              </div>
            </div>
            <div class="field-group">
              <label class="field-label">Embedding Model File (GGUF)</label>
              {#if localEmbeddingModelOptions.length > 0}
                <select bind:value={localEmbeddingModelPath} class="field-select">
                  <option value="">Auto-detect (only embedding GGUF file in folder)</option>
                  {#each localEmbeddingModelOptions as m (m.file_name)}
                    <option value={m.file_name}>{m.file_name} ({fmtModelSize(m.size_bytes)})</option>
                  {/each}
                </select>
                <p class="field-hint">
                  Powers semantic search, indexing, and "Summarize this Page" — separate from the
                  chat model above.
                </p>
              {:else}
                <p class="field-hint warning">
                  No embedding GGUF file found yet, so semantic search / "Summarize this Page"
                  is disabled. Download one (e.g. nomic-embed-text-v1.5-GGUF or
                  bge-small-en-v1.5-gguf from Hugging Face) into the Models Directory above, then
                  hit Refresh.
                </p>
              {/if}
            </div>
          {:else}
            <!-- Ollama / vLLM-OpenAI-compatible: a real endpoint to reach. -->
            <div class="field-group">
              <label class="field-label">Base URL</label>
              <input type="text" bind:value={localBaseUrl} class="field-input" placeholder={LOCAL_BASE_URL_DEFAULTS[localProvider]} />
            </div>
            {#if localProvider === "openai_compatible"}
              <div class="field-group">
                <label class="field-label">API Key (optional)</label>
                <input type="password" bind:value={localApiKey} class="field-input" placeholder="****** if endpoint requires auth" />
              </div>
            {/if}
            <div class="field-group">
              <label class="field-label">LLM Model</label>
              <input type="text" bind:value={llmModel} class="field-input" placeholder="qwen2.5-coder-14b-instruct-awq, llama3.2, etc." />
            </div>
            <div class="field-group">
              <label class="field-label">Embedding Model</label>
              <input type="text" bind:value={embeddingModel} class="field-input" placeholder="nomic-embed-text" />
            </div>
          {/if}
        </div>
      {/if}

      <!-- Cloud settings -->
      {#if mode === "cloud"}
        <div class="settings-section">
          <h4>Cloud Provider</h4>
          <div class="field-group">
            <label class="field-label">Provider</label>
            <div class="choice-row">
              <button class="choice-btn" class:active={cloudProvider === "openai"} onclick={() => (cloudProvider = "openai")}>OpenAI</button>
              <button class="choice-btn" class:active={cloudProvider === "anthropic"} onclick={() => (cloudProvider = "anthropic")}>Anthropic</button>
              <button class="choice-btn" class:active={cloudProvider === "openai_compatible"} onclick={() => (cloudProvider = "openai_compatible")}>vLLM / OpenAI-compatible</button>
            </div>
          </div>
          <div class="field-group">
            <label class="field-label">Cloud Base URL (optional)</label>
            <input type="text" bind:value={cloudBaseUrl} class="field-input" placeholder="Leave empty for official provider endpoint" />
          </div>
          <div class="field-group">
            <label class="field-label">Model</label>
            <input type="text" bind:value={cloudLlmModel} class="field-input"
              placeholder={cloudProvider === "openai" ? "gpt-4o" : "claude-sonnet-4-20250514"} />
          </div>
          <div class="field-group">
            <label class="field-label">API Key</label>
            <input type="password" bind:value={cloudApiKey} class="field-input" placeholder="sk-..." />
          </div>
          <div class="field-group">
            <label class="field-label">Embedding Provider</label>
            <div class="choice-row">
              <button class="choice-btn" class:active={cloudEmbeddingProvider === "openai"} onclick={() => (cloudEmbeddingProvider = "openai")}>OpenAI</button>
              <button class="choice-btn" class:active={cloudEmbeddingProvider === "openai_compatible"} onclick={() => (cloudEmbeddingProvider = "openai_compatible")}>vLLM / OpenAI-compatible</button>
            </div>
          </div>
          <div class="field-group">
            <label class="field-label">Embedding Base URL (optional)</label>
            <input type="text" bind:value={cloudEmbeddingBaseUrl} class="field-input" placeholder="Defaults to cloud base URL" />
          </div>
          <div class="field-group">
            <label class="field-label">Embedding Model</label>
            <input type="text" bind:value={cloudEmbeddingModel} class="field-input" />
          </div>
          <div class="field-group">
            <label class="field-label">Embedding API Key (optional)</label>
            <input type="password" bind:value={cloudEmbeddingApiKey} class="field-input" placeholder="defaults to cloud API key" />
          </div>
        </div>
      {/if}


      <!-- Actions -->
      <div class="actions-section">
        <button class="action-btn primary" onclick={saveConfig} disabled={isSaving}>
          {isSaving ? "Saving..." : "Save Configuration"}
        </button>
        <button
          class="action-btn"
          onclick={indexAll}
          disabled={isIndexing || !health?.enabled || !health?.embedder_available}
          title={health?.enabled && !health?.embedder_available
            ? "No search embedder configured — pick Ollama or vLLM/OpenAI-compatible as the local provider (or a cloud provider) to enable this."
            : undefined}
        >
          {isIndexing ? "Indexing..." : "Index All Pages"}
        </button>
        <button class="action-btn" onclick={createSchemas}>
          Create Default Schemas
        </button>
      </div>

      {#if indexCount !== null}
        <div class="index-result">Indexed {indexCount} chunks into vector store.</div>
      {/if}
    {/if}

    <!-- Whisper transcription (video/audio import) — independent of the
         chat/search "Enable AI features" toggle above, since importing a
         video and transcribing it locally doesn't need chat or search at
         all, and is the thing "Import Video" reaches for regardless. -->
    <div class="settings-section">
      <h4>Whisper Transcription (video/audio import)</h4>
      <p class="field-hint">
        When importing a video/audio URL, Grafium scrapes existing captions
        first (fast, free). If none exist, it falls back to transcribing the
        audio locally with Whisper — no cloud service or API key involved.
      </p>
      <label class="toggle-row">
        <input type="checkbox" bind:checked={mediaEnabled} />
        <span>Fall back to local Whisper transcription when no captions exist</span>
      </label>
      {#if mediaEnabled}
        <div class="field-group">
          <label class="field-label">Models Directory</label>
          <div class="browse-row">
            <input type="text" bind:value={mediaModelsDir} class="field-input" placeholder="e.g. ~/Documents/models — shared folder to search for model files" />
            <button type="button" class="browse-btn" onclick={browseMediaModelsDir}>Browse...</button>
            <button type="button" class="browse-btn" onclick={refreshMediaModelOptions} title="Re-scan this folder">Refresh</button>
          </div>
          <p class="field-hint">
            Point this at a folder you already keep local models in so Grafium never duplicates
            multi-gigabyte model files. Leave blank to use Grafium's own managed models folder
            (the same one Embedded local chat uses).
          </p>
        </div>
        <div class="field-group">
          <label class="field-label">Whisper Model File</label>
          {#if mediaModelOptions.length > 0}
            <select bind:value={mediaModelPath} class="field-select">
              <option value="">Auto-detect (only Whisper model in folder)</option>
              {#each mediaModelOptions as m (m.file_name)}
                <option value={m.file_name}>{m.file_name} ({fmtModelSize(m.size_bytes)})</option>
              {/each}
            </select>
          {:else}
            <p class="field-hint">
              No Whisper model files found yet in the Models Directory above. Download one there
              (e.g. ggml-base.en.bin), then hit Refresh.
            </p>
          {/if}
        </div>
        <div class="field-group">
          <label class="field-label">Language (optional)</label>
          <input type="text" bind:value={mediaLanguage} class="field-input" placeholder="en — leave blank to auto-detect" />
        </div>
      {/if}
      <div class="actions-section">
        <button class="action-btn primary" onclick={saveMediaConfig} disabled={isSavingMedia}>
          {isSavingMedia ? "Saving..." : "Save Transcription Settings"}
        </button>
      </div>
    </div>

    {#if message}
      <div class="message" class:error={messageType === "error"}>
        {message}
      </div>
    {/if}
  {/if}
</div>

<style>
  .ai-settings {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  h3 {
    font-size: 16px;
    font-weight: 600;
    margin: 0;
    color: var(--text-primary, #fff);
  }

  h4 {
    font-size: 13px;
    font-weight: 600;
    margin: 0 0 8px 0;
    color: var(--text-secondary, #aaa);
  }

  .loading {
    color: var(--text-muted, #888);
    font-size: 13px;
    padding: 12px 0;
  }

  .status-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    background: var(--bg-tertiary, #252535);
    border-radius: 6px;
    font-size: 13px;
    color: var(--text-muted, #888);
  }

  .status-bar.active {
    color: var(--text-primary, #fff);
  }

  .status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #666;
  }

  .status-bar.active .status-dot {
    background: #4ade80;
  }

  .status-vectors {
    margin-left: auto;
    font-size: 11px;
    opacity: 0.7;
  }

  .status-vectors.warning {
    color: #fbbf24;
    opacity: 1;
  }

  .toggle-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 14px;
    cursor: pointer;
    color: var(--text-primary, #fff);
  }

  .toggle-row input[type="checkbox"] {
    width: 16px;
    height: 16px;
    accent-color: var(--accent-color, #7c3aed);
  }

  .settings-section {
    background: var(--bg-tertiary, #252535);
    border-radius: 8px;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .field-group {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .field-label {
    font-size: 12px;
    color: var(--text-muted, #888);
  }

  .field-hint {
    font-size: 11px;
    color: var(--text-muted, #888);
    margin: 2px 0 0;
    line-height: 1.4;
  }

  .field-hint.warning {
    color: #fbbf24;
  }

  .field-hint.llm-load-error {
    white-space: pre-wrap;
    margin-top: 6px;
  }

  .field-input {
    background: var(--bg-secondary, #1e1e2e);
    border: 1px solid var(--border-color, #333);
    color: var(--text-primary, #fff);
    padding: 8px 10px;
    border-radius: 6px;
    font-size: 13px;
    outline: none;
  }

  .field-group-row {
    display: flex;
    flex-wrap: wrap;
    gap: 16px;
  }

  .field-subgroup {
    flex: 1 1 220px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .choice-row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .choice-btn {
    background: var(--bg-input, #252536);
    border: 1px solid var(--border, #333);
    color: var(--text-primary, #fff);
    border-radius: 6px;
    padding: 7px 10px;
    font-size: 12px;
    cursor: pointer;
  }

  .choice-btn.active {
    background: color-mix(in srgb, var(--accent, #7c3aed) 22%, var(--bg-input, #252536));
    border-color: var(--accent, #7c3aed);
  }

  .field-input:focus {
    border-color: var(--accent-color, #7c3aed);
  }

  .field-select {
    background: var(--bg-secondary, #1e1e2e);
    border: 1px solid var(--border-color, #333);
    color: var(--text-primary, #fff);
    padding: 8px 10px;
    border-radius: 6px;
    font-size: 13px;
    outline: none;
  }

  .field-select:focus {
    border-color: var(--accent-color, #7c3aed);
  }

  .model-picker {
    display: flex;
    gap: 10px;
    height: 220px;
  }

  .model-listbox {
    flex: 1 1 55%;
    min-width: 0;
    overflow-y: auto;
    background: var(--bg-secondary, #1e1e2e);
    border: 1px solid var(--border-color, #333);
    border-radius: 6px;
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .model-option {
    display: flex;
    align-items: center;
    gap: 6px;
    background: transparent;
    border: 1px solid transparent;
    color: var(--text-primary, #fff);
    border-radius: 4px;
    padding: 6px 8px;
    font-size: 12px;
    text-align: left;
    cursor: pointer;
    width: 100%;
  }

  .model-option:hover {
    background: var(--bg-hover, #2a2a3e);
  }

  .model-option.active {
    background: color-mix(in srgb, var(--accent, #7c3aed) 22%, var(--bg-input, #252536));
    border-color: var(--accent, #7c3aed);
  }

  .model-option-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .model-option-size {
    flex: none;
    color: var(--text-muted, #888);
    font-size: 11px;
  }

  .model-warn-icon {
    flex: none;
  }

  /* Red ⚠️ next to a model that won't fit in the detected GPU's VRAM.
     Distinct color from the unstable-architecture warning so users can
     tell "this model won't run" vs "this model will run painfully
     slowly" at a glance. */
  .model-vram-warn {
    flex: none;
    color: #f87171;
  }

  /* Amber ⚡ next to a model that fits but only just — usable on GPU
     with headroom to spare, so we don't want to scare people off, but
     they should know it's not comfortable. */
  .model-vram-tight {
    flex: none;
    color: #fbbf24;
  }

  /* Banner above the model listbox naming which GPU we detected and
     how much VRAM it has. Kept small and low-contrast so it feels like
     hardware context, not a call-to-action. */
  .gpu-detected-banner {
    color: var(--text-muted, #888);
    font-size: 11px;
    margin: 0 0 8px;
    padding: 6px 8px;
    background: var(--bg-secondary, #1e1e2e);
    border: 1px solid var(--border-color, #333);
    border-radius: 6px;
    line-height: 1.4;
  }

  .gpu-detected-banner-unknown {
    color: #a78bfa;
  }

  .model-description {
    flex: 1 1 45%;
    min-width: 0;
    overflow-y: auto;
    background: var(--bg-secondary, #1e1e2e);
    border: 1px solid var(--border-color, #333);
    border-radius: 6px;
    padding: 10px;
    font-size: 12px;
  }

  .model-description-title {
    font-weight: 600;
    font-size: 12px;
    margin-bottom: 6px;
    word-break: break-all;
  }

  .model-description-text {
    color: var(--text-muted, #888);
    line-height: 1.4;
    margin: 0 0 6px;
  }

  .model-description-meta {
    color: var(--text-muted, #888);
    font-size: 11px;
    margin: 0;
  }

  .model-description-warning {
    color: #fbbf24;
    background: rgba(251, 191, 36, 0.1);
    border: 1px solid rgba(251, 191, 36, 0.3);
    border-radius: 6px;
    padding: 6px 8px;
    line-height: 1.4;
    margin: 0 0 8px;
  }

  /* The fit explanation lives just below the unstable-architecture
     warning (if any) and above the model description. Base style is
     neutral; the -wont/-tight/-ok modifiers colour it to match the
     picker icons so the two locations feel like the same message. */
  .model-description-fit {
    border-radius: 6px;
    padding: 6px 8px;
    line-height: 1.4;
    margin: 0 0 8px;
    font-size: 12px;
  }

  .model-description-fit-wont {
    color: #f87171;
    background: rgba(220, 38, 38, 0.1);
    border: 1px solid rgba(220, 38, 38, 0.3);
  }

  .model-description-fit-tight {
    color: #fbbf24;
    background: rgba(251, 191, 36, 0.1);
    border: 1px solid rgba(251, 191, 36, 0.3);
  }

  .model-description-fit-ok {
    color: #4ade80;
    background: rgba(74, 222, 128, 0.08);
    border: 1px solid rgba(74, 222, 128, 0.25);
  }

  .model-description-placeholder {
    color: var(--text-muted, #888);
    font-size: 12px;
  }

  .browse-row {
    display: flex;
    gap: 8px;
  }

  .browse-row .field-input {
    flex: 1;
  }

  .input-row {
    display: flex;
    gap: 6px;
    align-items: stretch;
  }

  .input-row .field-input {
    flex: 1;
    min-width: 0;
  }

  .input-row .browse-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .browse-btn {
    background: var(--bg-input, #252536);
    border: 1px solid var(--border, #333);
    color: var(--text-primary, #fff);
    border-radius: 6px;
    padding: 7px 10px;
    font-size: 12px;
    cursor: pointer;
    white-space: nowrap;
  }

  .browse-btn:hover {
    border-color: var(--accent, #7c3aed);
  }

  .actions-section {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .action-btn {
    background: var(--bg-tertiary, #252535);
    border: 1px solid var(--border-color, #333);
    color: var(--text-primary, #fff);
    padding: 8px 14px;
    border-radius: 6px;
    cursor: pointer;
    font-size: 13px;
    transition: background 0.15s;
  }

  .action-btn:hover:not(:disabled) {
    background: var(--bg-hover, #2a2a3e);
  }

  .action-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .action-btn.primary {
    background: var(--accent-color, #7c3aed);
    border-color: transparent;
  }

  .action-btn.primary:hover:not(:disabled) {
    opacity: 0.9;
  }

  .index-result {
    font-size: 12px;
    color: #4ade80;
    padding: 4px 0;
  }

  .message {
    font-size: 12px;
    padding: 8px 12px;
    border-radius: 6px;
    background: rgba(74, 222, 128, 0.1);
    border: 1px solid rgba(74, 222, 128, 0.3);
    color: #4ade80;
  }

  .message.error {
    background: rgba(220, 38, 38, 0.1);
    border: 1px solid rgba(220, 38, 38, 0.3);
    color: #f87171;
  }
</style>
