<script lang="ts">
  import { onMount } from "svelte";
  import { listSpellLanguages, type SpellLanguage, type SpellcheckPreferences } from "../lib/api";
  import { spellcheck } from "../lib/spellcheck";
  import { describeError } from "../lib/toast.svelte";
  import SettingsHelp from "./SettingsHelp.svelte";

  const id = $props.id();

  let languages = $state<SpellLanguage[]>([]);
  let preferences = $state<SpellcheckPreferences | null>(null);
  let words = $state<string[]>([]);
  let startupFailure = $state<string | null>(null);
  let problem = $state("");
  let loaded = $state(false);
  let busy = $state(false);
  let newWord = $state("");

  const selected = $derived(new Set(preferences?.languages ?? []));
  const nothingSelected = $derived(!!preferences?.enabled && preferences.languages.length === 0);

  /** The English name, when it adds something, and roughly what the dictionary costs in memory. */
  function detail(language: SpellLanguage): string {
    const name = language.name === language.nativeName ? [] : [language.name];
    return [...name, `~${language.memoryMb} MB`].join(" · ");
  }

  async function load() {
    await spellcheck.ready();
    preferences = spellcheck.current();
    startupFailure = spellcheck.loadError();

    const results = await Promise.allSettled([listSpellLanguages(), spellcheck.listWords()]);
    const [list, personal] = results;
    if (list.status === "fulfilled") languages = list.value;
    if (personal.status === "fulfilled") words = personal.value;
    const failed = results.filter((result): result is PromiseRejectedResult => result.status === "rejected");
    if (failed.length) {
      problem = `Could not load the spell check lists: ${failed.map((r) => describeError(r.reason)).join("; ")}`;
    }
    loaded = true;
  }

  onMount(() => {
    void load();
  });

  /** Runs one change at a time and reports a failure where the user is looking. */
  async function change(failure: string, action: () => Promise<void>): Promise<boolean> {
    busy = true;
    problem = "";
    try {
      await action();
      return true;
    } catch (error) {
      problem = `${failure}: ${describeError(error)}`;
      return false;
    } finally {
      busy = false;
    }
  }

  const SAVE_FAILED = "Could not save the spell check settings";

  async function setEnabled(event: Event) {
    const box = event.currentTarget as HTMLInputElement;
    const saved = await change(SAVE_FAILED, async () => {
      preferences = await spellcheck.setPreferences({ enabled: box.checked });
    });
    if (!saved) box.checked = preferences?.enabled ?? false;
  }

  async function setLanguage(code: string, event: Event) {
    const box = event.currentTarget as HTMLInputElement;
    const order = languages.map((language) => language.code);
    const rank = (value: string) => {
      const index = order.indexOf(value);
      return index < 0 ? order.length : index;
    };
    const next = box.checked
      ? [...new Set([...selected, code])].sort((a, b) => rank(a) - rank(b))
      : [...selected].filter((value) => value !== code);
    const saved = await change(SAVE_FAILED, async () => {
      preferences = await spellcheck.setPreferences({ languages: next });
    });
    if (!saved) box.checked = selected.has(code);
  }

  async function addWord(event: SubmitEvent) {
    event.preventDefault();
    const word = newWord.trim();
    if (!word || busy) return;
    await change("Could not add the word", async () => {
      words = await spellcheck.addWord(word);
      if (newWord.trim() === word) newWord = "";
    });
  }

  async function removeWord(word: string) {
    await change("Could not remove the word", async () => {
      words = await spellcheck.removeWord(word);
    });
  }

  async function retry() {
    await change("Could not read the spell check settings", async () => {
      await spellcheck.reload();
      preferences = spellcheck.current();
      startupFailure = spellcheck.loadError();
    });
  }
</script>

<div class="spellcheck-settings">
  {#if startupFailure}
    <div class="setting-row problem" role="alert">
      <span>Spell check is off because its settings could not be read: {startupFailure}</span>
      <button class="action" type="button" disabled={busy} onclick={retry}>Try again</button>
    </div>
  {/if}

  <div class="setting-row">
    <span class="setting-label">Check spelling</span>
    <span class="control">
      <label class="setting-checkbox">
        <input
          type="checkbox"
          checked={preferences?.enabled ?? false}
          disabled={!preferences || busy}
          onchange={setEnabled}
        />
        <span>Underline misspelled words while writing</span>
      </label>
      <SettingsHelp title="Spell check">
        <p>Spell check runs on this device with dictionaries built into Grafium. It works offline and sends nothing anywhere.</p>
        <p>Right-click an underlined word to pick a correction, add it to your dictionary, or ignore it until Grafium restarts. Underlines appear in the block you are editing.</p>
        <p>Code, links, tags, page properties and other markup are never checked, and neither are words with digits or words that mix alphabets.</p>
        <p>A word is accepted if any selected language written in the same alphabet accepts it, so English and Spanish can be selected together.</p>
        <p>A language only uses memory while it is selected, and the figure beside it is approximate. The Hindi and Arabic dictionaries are community-made and less complete than the others.</p>
        <p>Your own words are saved in Grafium’s settings folder rather than in a graph, so every graph shares them.</p>
      </SettingsHelp>
    </span>
  </div>

  <div class="setting-row top" role="group" aria-labelledby={`${id}-languages`}>
    <span class="setting-label" id={`${id}-languages`}>Languages</span>
    <div class="language-list">
      {#each languages as language (language.code)}
        <label class="setting-checkbox">
          <input
            type="checkbox"
            checked={selected.has(language.code)}
            disabled={!preferences?.enabled || busy}
            onchange={(event) => setLanguage(language.code, event)}
          />
          <span class="language-name" dir="auto">{language.nativeName}</span>
          <span class="language-meta">{detail(language)}</span>
        </label>
      {/each}
    </div>
  </div>

  {#if nothingSelected}
    <p class="setting-desc warning" role="status">No language is selected, so nothing is checked.</p>
  {/if}

  <div class="setting-row top">
    <label class="setting-label" for={`${id}-new-word`}>My dictionary</label>
    <div class="dictionary">
      <form class="add-word" onsubmit={addWord}>
        <input
          id={`${id}-new-word`}
          type="text"
          bind:value={newWord}
          placeholder="Add a word"
          maxlength="64"
          autocomplete="off"
          autocapitalize="off"
          spellcheck="false"
        />
        <button class="action" type="submit" disabled={busy || !newWord.trim()}>Add</button>
      </form>
      {#if words.length}
        <ul class="words" aria-label="Words in your dictionary">
          {#each words as word}
            <li>
              <span class="word" dir="auto">{word}</span>
              <button
                class="action"
                type="button"
                aria-label={`Remove ${word} from the dictionary`}
                disabled={busy}
                onclick={() => removeWord(word)}
              >Remove</button>
            </li>
          {/each}
        </ul>
      {:else if loaded}
        <p class="empty">No words added yet.</p>
      {/if}
    </div>
  </div>

  {#if problem}
    <p class="setting-desc problem" role="alert">{problem}</p>
  {/if}
</div>

<style>
  .spellcheck-settings {
    display: flex;
    flex-direction: column;
    gap: 16px;
    color: var(--text-primary);
  }

  .setting-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    flex-wrap: wrap;
    gap: 12px;
  }

  .setting-row.top {
    align-items: flex-start;
  }

  .setting-label {
    font-size: 13px;
    color: var(--text-secondary);
  }

  .control {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }

  .setting-checkbox {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-muted);
    cursor: pointer;
  }

  .setting-checkbox input {
    cursor: pointer;
  }

  .language-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }

  .language-name {
    color: var(--text-primary);
  }

  .language-meta {
    color: var(--text-muted);
    font-size: 11px;
  }

  .dictionary {
    display: flex;
    flex-direction: column;
    gap: 8px;
    flex: 1 1 220px;
    max-width: 360px;
    min-width: 0;
  }

  .add-word {
    display: flex;
    gap: 8px;
  }

  .add-word input {
    flex: 1 1 auto;
    min-width: 0;
    padding: 5px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-input);
    color: var(--text-primary);
    font: inherit;
    font-size: 12px;
  }

  input:focus-visible,
  button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  .action {
    padding: 5px 10px;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--bg-secondary);
    color: var(--text-secondary);
    font: inherit;
    font-size: 11px;
    font-weight: 500;
    cursor: pointer;
  }

  .action:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .action:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .words {
    max-height: 220px;
    margin: 0;
    padding: 0;
    overflow-y: auto;
    list-style: none;
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .words li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 8px;
    font-size: 12px;
  }

  .words li + li {
    border-top: 1px solid var(--border);
  }

  .word {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .empty,
  .setting-desc {
    margin: 0;
    font-size: 12px;
    color: var(--text-muted);
  }

  .warning {
    color: var(--text-secondary);
  }

  .problem {
    color: var(--danger);
  }
</style>
