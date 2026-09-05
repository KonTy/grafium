<script lang="ts">
  // Research settings — the search-engine registry a student can extend, plus
  // the editable prompt for every step of the agentic research workflow. Visual
  // language deliberately mirrors AISettings.svelte (same field-group /
  // field-label / field-hint / settings-section conventions) so Settings reads
  // as one coherent surface.
  import {
    researchGetConfig,
    researchSetConfig,
    researchResetPrompts,
    researchTestEngine,
    defaultResearchConfig,
    clampResearchNumbers,
    validateEngineDraft,
    engineFromDraft,
    DEFAULT_RESEARCH_PROMPTS,
    RESEARCH_PROMPT_STEPS,
    RESEARCH_LIMITS,
    type ResearchConfig,
    type ResearchPrompts,
    type SearchEngineDef,
    type SearchResult,
    type EngineDraft,
    type EngineKind,
    type EngineCategory,
  } from "../lib/research";

  // Working copy the form binds to directly. Null only until the first load
  // resolves; every render of the form is guarded by `{#if config}`.
  let config = $state<ResearchConfig | null>(null);
  let isLoading = $state(true);
  let isSaving = $state(false);
  // False when `research_get_config` isn't registered yet: we still render the
  // built-in defaults so the page isn't blank, but saving/testing would fail, so
  // those are disabled and a banner explains why.
  let backendAvailable = $state(true);

  let message = $state("");
  let messageType = $state<"success" | "error">("success");

  // The query the per-engine Test button runs. A neutral topic that returns hits
  // from both web and academic sources, so a student testing a fresh engine gets
  // a meaningful result instead of an empty page.
  let testQuery = $state("photosynthesis");

  type TestOutcome =
    | { status: "running" }
    | { status: "ok"; results: SearchResult[] }
    | { status: "error"; error: string };
  // Keyed by engine id so each row shows its own outcome inline.
  let testResults = $state<Record<string, TestOutcome>>({});

  // Add-engine form. Both selector blocks are pre-seeded so binding works before
  // a kind is chosen; `engineFromDraft` drops whichever block doesn't apply.
  function emptyDraft(): EngineDraft {
    return {
      id: "",
      name: "",
      kind: "Html",
      url_template: "",
      category: "Web",
      selectors: { result: "", link: "", title: "", snippet: "" },
      json_paths: { results: "", url: "", title: "", snippet: "", url_prefix: "" },
    };
  }
  let draft = $state<EngineDraft>(emptyDraft());
  let showAdd = $state(false);

  let existingIds = $derived(config ? config.engines.map((e) => e.id) : []);
  let draftErrors = $derived(validateEngineDraft(draft, existingIds));
  let enabledCount = $derived(config ? config.engines.filter((e) => e.enabled).length : 0);

  $effect(() => {
    void loadConfig();
  });

  async function loadConfig() {
    isLoading = true;
    try {
      config = await researchGetConfig();
      backendAvailable = true;
    } catch {
      // Backend command not registered yet — fall back to the contract defaults
      // so the page is usable and the shape is visible.
      config = defaultResearchConfig();
      backendAvailable = false;
    } finally {
      isLoading = false;
    }
  }

  async function saveConfig() {
    if (!config) return;
    isSaving = true;
    try {
      await researchSetConfig(clampResearchNumbers($state.snapshot(config)));
      showMessage("Research settings saved.", "success");
    } catch (e: any) {
      showMessage("Failed to save: " + e, "error");
    } finally {
      isSaving = false;
    }
  }

  async function revert() {
    await loadConfig();
    testResults = {};
    showMessage("Reverted to saved settings.", "success");
  }

  async function testEngine(engine: SearchEngineDef) {
    testResults = { ...testResults, [engine.id]: { status: "running" } };
    try {
      const results = await researchTestEngine($state.snapshot(engine), testQuery.trim() || "test");
      testResults = { ...testResults, [engine.id]: { status: "ok", results } };
    } catch (e: any) {
      testResults = { ...testResults, [engine.id]: { status: "error", error: String(e) } };
    }
  }

  function deleteEngine(engine: SearchEngineDef) {
    // Built-ins are disable-only; the button is disabled, but guard here too.
    if (engine.builtin || !config) return;
    config.engines = config.engines.filter((e) => e.id !== engine.id);
    const next = { ...testResults };
    delete next[engine.id];
    testResults = next;
  }

  function addEngine() {
    if (!config || draftErrors.length > 0) return;
    config.engines = [...config.engines, engineFromDraft($state.snapshot(draft))];
    draft = emptyDraft();
    showAdd = false;
    showMessage("Engine added — Save to keep it.", "success");
  }

  function resetPrompt(key: keyof ResearchPrompts) {
    if (config) config.prompts[key] = DEFAULT_RESEARCH_PROMPTS[key];
  }

  async function resetAllPrompts() {
    if (!config) return;
    if (backendAvailable) {
      try {
        const fresh = await researchResetPrompts();
        config.prompts = fresh.prompts;
        showMessage("Prompts reset to defaults.", "success");
        return;
      } catch {
        // Fall through to the UI's bundled defaults.
      }
    }
    config.prompts = { ...DEFAULT_RESEARCH_PROMPTS };
    showMessage("Prompts reset to defaults.", "success");
  }

  function showMessage(msg: string, type: "success" | "error") {
    message = msg;
    messageType = type;
    setTimeout(() => (message = ""), 4000);
  }

  // Title-only preview of the first few Test hits — enough for a student to see
  // whether the selector actually matched anything sensible.
  function previewTitles(results: SearchResult[]): string[] {
    return results.slice(0, 3).map((r) => (r.title || r.url || "(untitled)").trim());
  }
</script>

<div class="research-settings">
  <h3>Research</h3>
  <p class="field-hint">
    Configure the search engines the deep-research workflow uses, and edit the
    prompt for each step of that workflow. This is the same workflow the
    <strong>Research</strong> checkbox under the Chat box runs.
  </p>

  {#if isLoading}
    <div class="loading">Loading research settings…</div>
  {:else if config}
    {#if !backendAvailable}
      <div class="message error" role="status">
        The research backend isn't available in this build yet, so these are the
        built-in defaults. You can look around, but saving and testing are
        disabled until it's wired up.
      </div>
    {/if}

    <!-- ── Engines ─────────────────────────────────────────────────────── -->
    <div class="settings-section">
      <h4>Search engines</h4>
      <p class="field-hint">
        Each engine's query URL uses a <code>{"{query}"}</code> placeholder — it's
        replaced with your search terms (URL-encoded) when the engine runs.
        Built-in engines can be turned off but not deleted.
      </p>

      {#if enabledCount === 0}
        <p class="field-hint warning">
          No engines are enabled — research has nothing to search. Enable at least
          one below.
        </p>
      {/if}

      <div class="field-group test-query-row">
        <label class="field-label" for="research-test-query">Test query</label>
        <input
          id="research-test-query"
          type="text"
          class="field-input"
          bind:value={testQuery}
          placeholder="A topic to try the engines with"
        />
      </div>

      <ul class="engine-list">
        {#each config.engines as engine (engine.id)}
          <li class="engine-item">
            <div class="engine-row">
              <label class="engine-enable">
                <input type="checkbox" bind:checked={engine.enabled} />
                <span class="engine-name">{engine.name}</span>
              </label>
              <span class="engine-tag" class:academic={engine.category === "Academic"}>
                {engine.category}
              </span>
              <span class="engine-tag kind">{engine.kind}</span>
              {#if engine.builtin}
                <span class="engine-tag builtin">Built-in</span>
              {/if}
              <div class="engine-actions">
                <button
                  type="button"
                  class="action-btn"
                  onclick={() => testEngine(engine)}
                  disabled={!backendAvailable || testResults[engine.id]?.status === "running"}
                  title={backendAvailable ? "Run this engine once and show what it returns" : "Unavailable until the research backend is wired up"}
                >
                  {testResults[engine.id]?.status === "running" ? "Testing…" : "Test"}
                </button>
                <button
                  type="button"
                  class="action-btn danger"
                  onclick={() => deleteEngine(engine)}
                  disabled={engine.builtin}
                  title={engine.builtin ? "Built-in engines can be disabled but not deleted" : "Delete this engine"}
                >
                  Delete
                </button>
              </div>
            </div>

            {#if testResults[engine.id]}
              {@const outcome = testResults[engine.id]}
              <div class="engine-test">
                {#if outcome.status === "running"}
                  <span class="test-muted">Running “{testQuery}”…</span>
                {:else if outcome.status === "ok"}
                  {#if outcome.results.length === 0}
                    <span class="test-warn">
                      0 results. The engine responded but nothing was parsed — usually a
                      wrong selector/path rather than a network problem.
                    </span>
                  {:else}
                    <span class="test-ok">{outcome.results.length} result{outcome.results.length === 1 ? "" : "s"}</span>
                    <ul class="test-titles">
                      {#each previewTitles(outcome.results) as title}
                        <li>{title}</li>
                      {/each}
                    </ul>
                  {/if}
                {:else}
                  <span class="test-err">Error: {outcome.error}</span>
                {/if}
              </div>
            {/if}
          </li>
        {/each}
      </ul>

      {#if !showAdd}
        <div class="actions-section">
          <button type="button" class="action-btn" onclick={() => (showAdd = true)}>+ Add engine</button>
        </div>
      {:else}
        <div class="add-form">
          <h5>Add a search engine</h5>
          <div class="field-row">
            <div class="field-group">
              <label class="field-label" for="add-eng-id">ID (slug)</label>
              <input id="add-eng-id" type="text" class="field-input" bind:value={draft.id} placeholder="my-engine" />
            </div>
            <div class="field-group">
              <label class="field-label" for="add-eng-name">Display name</label>
              <input id="add-eng-name" type="text" class="field-input" bind:value={draft.name} placeholder="My Engine" />
            </div>
          </div>

          <div class="field-group">
            <span class="field-label" id="add-eng-kind-label">Kind</span>
            <div class="choice-row" role="group" aria-labelledby="add-eng-kind-label">
              <button type="button" class="choice-btn" class:active={draft.kind === "Html"} onclick={() => (draft.kind = "Html" as EngineKind)}>HTML (scrape a results page)</button>
              <button type="button" class="choice-btn" class:active={draft.kind === "Json"} onclick={() => (draft.kind = "Json" as EngineKind)}>JSON (call an API)</button>
            </div>
          </div>

          <div class="field-group">
            <span class="field-label" id="add-eng-cat-label">Category</span>
            <div class="choice-row" role="group" aria-labelledby="add-eng-cat-label">
              <button type="button" class="choice-btn" class:active={draft.category === "Web"} onclick={() => (draft.category = "Web" as EngineCategory)}>Web</button>
              <button type="button" class="choice-btn" class:active={draft.category === "Academic"} onclick={() => (draft.category = "Academic" as EngineCategory)}>Academic</button>
            </div>
          </div>

          <div class="field-group">
            <label class="field-label" for="add-eng-url">Query URL</label>
            <input id="add-eng-url" type="text" class="field-input" bind:value={draft.url_template} placeholder="https://example.com/search?q={'{query}'}" />
            <p class="field-hint">Must contain <code>{"{query}"}</code> where the search terms go.</p>
          </div>

          {#if draft.kind === "Html" && draft.selectors}
            <div class="field-row">
              <div class="field-group">
                <label class="field-label" for="add-eng-sel-result">Result selector</label>
                <input id="add-eng-sel-result" type="text" class="field-input" bind:value={draft.selectors.result} placeholder=".result" />
              </div>
              <div class="field-group">
                <label class="field-label" for="add-eng-sel-link">Link selector</label>
                <input id="add-eng-sel-link" type="text" class="field-input" bind:value={draft.selectors.link} placeholder="a.result__link" />
              </div>
            </div>
            <div class="field-row">
              <div class="field-group">
                <label class="field-label" for="add-eng-sel-title">Title selector</label>
                <input id="add-eng-sel-title" type="text" class="field-input" bind:value={draft.selectors.title} placeholder="h2" />
              </div>
              <div class="field-group">
                <label class="field-label" for="add-eng-sel-snippet">Snippet selector (optional)</label>
                <input id="add-eng-sel-snippet" type="text" class="field-input" bind:value={draft.selectors.snippet} placeholder=".snippet" />
              </div>
            </div>
            <p class="field-hint">CSS selectors, relative to each result container.</p>
          {:else if draft.kind === "Json" && draft.json_paths}
            <div class="field-row">
              <div class="field-group">
                <label class="field-label" for="add-eng-path-results">Results path</label>
                <input id="add-eng-path-results" type="text" class="field-input" bind:value={draft.json_paths.results} placeholder="data" />
              </div>
              <div class="field-group">
                <label class="field-label" for="add-eng-path-url">URL path</label>
                <input id="add-eng-path-url" type="text" class="field-input" bind:value={draft.json_paths.url} placeholder="url" />
              </div>
            </div>
            <div class="field-row">
              <div class="field-group">
                <label class="field-label" for="add-eng-path-title">Title path</label>
                <input id="add-eng-path-title" type="text" class="field-input" bind:value={draft.json_paths.title} placeholder="title" />
              </div>
              <div class="field-group">
                <label class="field-label" for="add-eng-path-snippet">Snippet path (optional)</label>
                <input id="add-eng-path-snippet" type="text" class="field-input" bind:value={draft.json_paths.snippet} placeholder="abstract" />
              </div>
            </div>
            <div class="field-row">
              <div class="field-group">
                <label class="field-label" for="add-eng-path-url-prefix">URL prefix (optional)</label>
                <input id="add-eng-path-url-prefix" type="text" class="field-input" bind:value={draft.json_paths.url_prefix} placeholder="https://openlibrary.org" />
              </div>
            </div>
            <p class="field-hint">Dotted paths into the JSON response, e.g. <code>message.items</code> then <code>title.0</code>. Set <strong>URL prefix</strong> only when the API returns relative URLs like <code>/works/OL123W</code> — it's prepended so each citation is an absolute, openable link.</p>
          {/if}

          {#if draftErrors.length > 0}
            <ul class="draft-errors">
              {#each draftErrors as err}
                <li>{err}</li>
              {/each}
            </ul>
          {/if}

          <div class="actions-section">
            <button type="button" class="action-btn primary" onclick={addEngine} disabled={draftErrors.length > 0}>Add engine</button>
            <button type="button" class="action-btn" onclick={() => { showAdd = false; draft = emptyDraft(); }}>Cancel</button>
          </div>
        </div>
      {/if}
    </div>

    <!-- ── Workflow ────────────────────────────────────────────────────── -->
    <div class="settings-section">
      <h4>Workflow</h4>
      <p class="field-hint">
        These prompts drive the agentic research loop, shown in the order they
        run. Edit them to change how each step behaves.
      </p>

      {#each RESEARCH_PROMPT_STEPS as step, i (step.key)}
        <div class="field-group prompt-block">
          <div class="prompt-head">
            <label class="field-label prompt-label" for={`research-prompt-${step.key}`}>
              {i + 1}. {step.label}
            </label>
            <button type="button" class="link-btn" onclick={() => resetPrompt(step.key)}>Reset to default</button>
          </div>
          <p class="field-hint">{step.explanation}</p>
          <textarea
            id={`research-prompt-${step.key}`}
            class="field-textarea"
            rows="4"
            bind:value={config.prompts[step.key]}
          ></textarea>
        </div>
      {/each}

      <div class="actions-section">
        <button type="button" class="action-btn" onclick={resetAllPrompts}>Reset all prompts to defaults</button>
      </div>

      <div class="field-row limits-row">
        <div class="field-group">
          <label class="field-label" for="research-max-rounds">Max rounds</label>
          <input
            id="research-max-rounds"
            type="number"
            class="field-input"
            min={RESEARCH_LIMITS.max_rounds.min}
            max={RESEARCH_LIMITS.max_rounds.max}
            bind:value={config.max_rounds}
          />
          <p class="field-hint">How many search-and-refine rounds it may run before answering.</p>
        </div>
        <div class="field-group">
          <label class="field-label" for="research-max-sources">Max sources</label>
          <input
            id="research-max-sources"
            type="number"
            class="field-input"
            min={RESEARCH_LIMITS.max_sources.min}
            max={RESEARCH_LIMITS.max_sources.max}
            bind:value={config.max_sources}
          />
          <p class="field-hint">The most sources it will read in full across the whole run.</p>
        </div>
        <div class="field-group">
          <label class="field-label" for="research-results-per-query">Results per query</label>
          <input
            id="research-results-per-query"
            type="number"
            class="field-input"
            min={RESEARCH_LIMITS.results_per_query.min}
            max={RESEARCH_LIMITS.results_per_query.max}
            bind:value={config.results_per_query}
          />
          <p class="field-hint">How many hits each search pulls back before choosing what to read.</p>
        </div>
      </div>

      <label class="toggle-row">
        <input type="checkbox" bind:checked={config.ocr_enabled} />
        <span>Read text inside scanned PDFs and images (OCR)</span>
      </label>
      <p class="field-hint">
        Needs the <code>tesseract</code> tool installed on your system. If it isn't
        found, OCR is simply skipped and the rest of the research still runs.
      </p>
    </div>

    <div class="actions-section">
      <button type="button" class="action-btn primary" onclick={saveConfig} disabled={isSaving || !backendAvailable}>
        {isSaving ? "Saving…" : "Save research settings"}
      </button>
      <button type="button" class="action-btn" onclick={revert} disabled={isSaving}>Revert</button>
    </div>

    {#if message}
      <div class="message" class:error={messageType === "error"}>{message}</div>
    {/if}
  {/if}
</div>

<style>
  .research-settings {
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

  h5 {
    font-size: 12px;
    font-weight: 600;
    margin: 0 0 4px 0;
    color: var(--text-secondary, #aaa);
  }

  .loading {
    color: var(--text-muted, #888);
    font-size: 13px;
    padding: 12px 0;
  }

  .settings-section {
    background: var(--bg-secondary, #252535);
    border: 1px solid var(--border, #333);
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
    color: var(--accent-yellow, #fbbf24);
  }

  .field-input {
    background: var(--bg-input, #1e1e2e);
    border: 1px solid var(--border, #333);
    color: var(--text-primary, #fff);
    padding: 8px 10px;
    border-radius: 6px;
    font-size: 13px;
    outline: none;
  }

  .field-input:focus {
    border-color: var(--accent, #7c3aed);
  }

  .field-textarea {
    background: var(--bg-input, #1e1e2e);
    border: 1px solid var(--border, #333);
    color: var(--text-primary, #fff);
    padding: 8px 10px;
    border-radius: 6px;
    font-size: 12px;
    line-height: 1.45;
    font-family: inherit;
    resize: vertical;
    min-height: 72px;
    outline: none;
  }

  .field-textarea:focus {
    border-color: var(--accent, #7c3aed);
  }

  .field-row {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
  }

  .field-row .field-group {
    flex: 1 1 180px;
  }

  .limits-row .field-group {
    flex: 1 1 140px;
  }

  code {
    background: var(--bg-code, rgba(127, 127, 127, 0.18));
    border-radius: 4px;
    padding: 0 4px;
    font-size: 0.92em;
  }

  /* Engines list */
  .engine-list {
    list-style: none;
    margin: 4px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .engine-item {
    border: 1px solid var(--border, #333);
    border-radius: 6px;
    padding: 8px 10px;
    background: var(--bg-input, #1e1e2e);
  }

  .engine-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .engine-enable {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    color: var(--text-primary, #fff);
    font-size: 13px;
  }

  .engine-enable input[type="checkbox"] {
    width: 16px;
    height: 16px;
    accent-color: var(--accent, #7c3aed);
  }

  .engine-name {
    font-weight: 500;
  }

  .engine-tag {
    font-size: 10px;
    letter-spacing: 0.02em;
    padding: 1px 7px;
    border-radius: 999px;
    border: 1px solid color-mix(in srgb, var(--accent-blue, #60a5fa) 45%, transparent);
    color: var(--accent-blue, #60a5fa);
    background: color-mix(in srgb, var(--accent-blue, #60a5fa) 12%, transparent);
  }

  .engine-tag.academic {
    border-color: color-mix(in srgb, var(--accent-purple, #a78bfa) 45%, transparent);
    color: var(--accent-purple, #a78bfa);
    background: color-mix(in srgb, var(--accent-purple, #a78bfa) 12%, transparent);
  }

  .engine-tag.kind,
  .engine-tag.builtin {
    border-color: var(--border, #444);
    color: var(--text-muted, #999);
    background: transparent;
  }

  .engine-actions {
    margin-left: auto;
    display: flex;
    gap: 6px;
  }

  .engine-test {
    margin-top: 8px;
    padding-top: 8px;
    border-top: 1px dashed var(--border, #333);
    font-size: 12px;
    color: var(--text-muted, #999);
  }

  .test-ok {
    color: var(--accent-green, #4ade80);
    font-weight: 600;
  }

  .test-warn {
    color: var(--accent-yellow, #fbbf24);
  }

  .test-err {
    color: var(--accent-red, #f87171);
  }

  .test-titles {
    margin: 4px 0 0;
    padding-left: 18px;
    color: var(--text-secondary, #bbb);
  }

  .test-titles li {
    margin: 1px 0;
  }

  /* Add-engine form */
  .add-form {
    border: 1px solid var(--border, #333);
    border-radius: 8px;
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    background: color-mix(in srgb, var(--accent, #7c3aed) 6%, transparent);
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

  .draft-errors {
    margin: 0;
    padding-left: 18px;
    font-size: 11px;
    color: var(--accent-yellow, #fbbf24);
  }

  /* Prompt editor */
  .prompt-block {
    border-top: 1px solid var(--border, #333);
    padding-top: 10px;
  }

  .prompt-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }

  .prompt-label {
    font-size: 13px;
    color: var(--text-primary, #fff);
    font-weight: 500;
  }

  .link-btn {
    background: none;
    border: none;
    color: var(--accent, #7c3aed);
    cursor: pointer;
    font-size: 11px;
    padding: 2px 4px;
    border-radius: 4px;
  }

  .link-btn:hover {
    text-decoration: underline;
  }

  .toggle-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 14px;
    cursor: pointer;
    color: var(--text-primary, #fff);
    margin-top: 4px;
  }

  .toggle-row input[type="checkbox"] {
    width: 16px;
    height: 16px;
    accent-color: var(--accent, #7c3aed);
  }

  /* Actions */
  .actions-section {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .action-btn {
    background: var(--bg-input, #252535);
    border: 1px solid var(--border, #333);
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
    background: var(--accent, #7c3aed);
    border-color: transparent;
    color: var(--btn-primary-fg, #fff);
  }

  .action-btn.primary:hover:not(:disabled) {
    opacity: 0.9;
  }

  .action-btn.danger:hover:not(:disabled) {
    border-color: var(--accent-red, #f87171);
    color: var(--accent-red, #f87171);
  }

  .message {
    font-size: 12px;
    padding: 8px 12px;
    border-radius: 6px;
    background: color-mix(in srgb, var(--accent-green, #4ade80) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent-green, #4ade80) 30%, transparent);
    color: var(--accent-green, #4ade80);
  }

  .message.error {
    background: color-mix(in srgb, var(--accent-red, #f87171) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent-red, #f87171) 30%, transparent);
    color: var(--accent-red, #f87171);
  }
</style>
