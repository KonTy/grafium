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
  import { mediaGetConfig, mediaSetConfig, listLocalModels, type MediaConfigPayload, type LocalModelInfo } from "../lib/api";

  let config = $state<AiConfig | null>(null);
  let health = $state<HealthStatus | null>(null);
  let isLoading = $state(false);
  let isSaving = $state(false);
  let isIndexing = $state(false);
  let indexCount = $state<number | null>(null);
  let message = $state("");
  let messageType = $state<"success" | "error">("success");

  // Form state
  let enabled = $state(false);
  let mode = $state("local");
  let localProvider = $state("openai_compatible");
  let localBaseUrl = $state("http://localhost:8000/v1");
  let localApiKey = $state("");
  let localModelPath = $state("");
  let localEmbeddingModelPath = $state("");
  let localModelsDir = $state("");
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
      const result = await aiIndexAllPages();
      indexCount = result.indexed_chunks;
      const failed =
        result.pages_failed > 0 ? `, ${result.pages_failed} page(s) failed` : "";
      showMessage(
        `Indexed ${result.indexed_chunks} chunks from ${result.pages_processed} page(s)${failed}`,
        result.pages_failed > 0 ? "error" : "success"
      );
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
              {#if localModelOptions.length > 0}
                <select bind:value={localModelPath} class="field-select">
                  <option value="">Auto-detect (only chat GGUF file in folder)</option>
                  {#each localModelOptions as m (m.file_name)}
                    <option value={m.file_name}>{m.file_name} ({fmtModelSize(m.size_bytes)})</option>
                  {/each}
                </select>
              {:else}
                <p class="field-hint">
                  No chat GGUF files found yet in the Models Directory above. Download one there,
                  then hit Refresh — it'll show up here instead of needing to be typed by hand.
                </p>
              {/if}
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
                  Powers semantic search, indexing, and "Analyze this Page" — separate from the
                  chat model above.
                </p>
              {:else}
                <p class="field-hint warning">
                  No embedding GGUF file found yet, so semantic search / "Analyze this Page"
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

  .field-input {
    background: var(--bg-secondary, #1e1e2e);
    border: 1px solid var(--border-color, #333);
    color: var(--text-primary, #fff);
    padding: 8px 10px;
    border-radius: 6px;
    font-size: 13px;
    outline: none;
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

  .browse-row {
    display: flex;
    gap: 8px;
  }

  .browse-row .field-input {
    flex: 1;
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
