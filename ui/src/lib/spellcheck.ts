import {
  addPersonalWord,
  checkSpelling,
  getSpellcheckPreferences,
  listPersonalWords,
  removePersonalWord,
  saveSpellcheckPreferences,
  suggestSpelling,
  type SpellcheckPreferences,
} from "./api";

/** The backend calls the service relies on; tests swap in a fake. */
export interface SpellApi {
  getPreferences(locale?: string): Promise<SpellcheckPreferences>;
  savePreferences(
    patch: Partial<SpellcheckPreferences>,
    locale?: string,
  ): Promise<SpellcheckPreferences>;
  check(words: string[], languages: string[]): Promise<boolean[]>;
  suggest(word: string, languages: string[], limit?: number): Promise<string[]>;
  listWords(): Promise<string[]>;
  addWord(word: string): Promise<string[]>;
  removeWord(word: string): Promise<string[]>;
}

const backend: SpellApi = {
  getPreferences: getSpellcheckPreferences,
  savePreferences: saveSpellcheckPreferences,
  check: checkSpelling,
  suggest: suggestSpelling,
  listWords: listPersonalWords,
  addWord: addPersonalWord,
  removeWord: removePersonalWord,
};

/** Words sent per call, so one huge page cannot stall the backend. */
const BATCH_SIZE = 500;
/** Verdicts remembered; the cache starts over when it grows past this. */
const CACHE_LIMIT = 50_000;
export const SUGGESTION_LIMIT = 5;

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Spell checking for the editors.
 *
 * Everything runs in the app's own process, so it works offline. When the
 * checker cannot start (for example its settings file is unreadable) the
 * editors simply show no squiggles: a broken spell checker must never get in
 * the way of writing, and a failed load never writes anything back.
 */
export class SpellcheckService {
  private readonly api: SpellApi;
  private readonly locale: () => string | undefined;
  private preferences: SpellcheckPreferences | null = null;
  private failure: string | null = null;
  private loading: Promise<void> | null = null;
  /** Bumped whenever earlier verdicts may no longer hold. */
  private epoch = 0;
  private readonly verdicts = new Map<string, boolean>();
  private readonly ignored = new Set<string>();
  private readonly listeners = new Set<() => void>();
  private readonly warned = new Set<string>();

  constructor(
    api: SpellApi = backend,
    locale: () => string | undefined = () =>
      typeof navigator === "undefined" ? undefined : navigator.language,
  ) {
    this.api = api;
    this.locale = locale;
  }

  /** Reads the settings once; every caller shares the result. */
  ready(): Promise<void> {
    this.loading ??= this.load();
    return this.loading;
  }

  /** Reads the settings again, for example after a failed start. */
  async reload(): Promise<void> {
    this.loading = this.load();
    await this.loading;
    this.invalidate();
  }

  private async load(): Promise<void> {
    try {
      this.preferences = await this.api.getPreferences(this.locale());
      this.failure = null;
    } catch (error) {
      this.preferences = null;
      this.failure = errorMessage(error);
      this.warn("load", "Spell check is off because its settings could not be read.", error);
    }
  }

  /** Why the settings could not be read, or `null` when they could. */
  loadError(): string | null {
    return this.failure;
  }

  /** The current settings; `null` until they have loaded or if they failed. */
  current(): SpellcheckPreferences | null {
    return this.preferences && {
      enabled: this.preferences.enabled,
      languages: [...this.preferences.languages],
    };
  }

  /** Whether words are being checked at all. */
  isActive(): boolean {
    return !!this.preferences?.enabled && this.preferences.languages.length > 0;
  }

  /** Saves the changed settings. Throws, leaving things as they were, if that fails. */
  async setPreferences(patch: Partial<SpellcheckPreferences>): Promise<SpellcheckPreferences> {
    await this.ready();
    this.preferences = await this.api.savePreferences(patch, this.locale());
    this.failure = null;
    this.invalidate();
    return this.current()!;
  }

  /**
   * The words in `words` that are misspelled.
   *
   * Never rejects: if the backend fails the affected words count as correct.
   * Verdicts for a configuration that changed while the call was running are
   * not kept; subscribers are told about the change and should check again.
   */
  async check(words: readonly string[]): Promise<Set<string>> {
    await this.ready();
    if (!this.isActive()) return new Set();

    const epoch = this.epoch;
    const languages = [...this.preferences!.languages];
    const answers = new Map<string, boolean>();
    const unknown: string[] = [];
    for (const word of new Set(words)) {
      const known = this.verdicts.get(word);
      if (known === undefined) unknown.push(word);
      else answers.set(word, known);
    }

    const fresh = new Map<string, boolean>();
    try {
      for (let at = 0; at < unknown.length; at += BATCH_SIZE) {
        const batch = unknown.slice(at, at + BATCH_SIZE);
        const flags = await this.api.check(batch, languages);
        batch.forEach((word, i) => fresh.set(word, flags[i] === true));
      }
    } catch (error) {
      this.warn("check", "Spell check could not check some words.", error);
    }

    if (epoch === this.epoch) this.remember(fresh);
    const misspelled = new Set<string>();
    for (const [word, bad] of [...answers, ...fresh]) {
      if (bad) misspelled.add(word);
    }
    return misspelled;
  }

  private remember(fresh: Map<string, boolean>): void {
    if (this.verdicts.size + fresh.size > CACHE_LIMIT) this.verdicts.clear();
    for (const [word, bad] of fresh) this.verdicts.set(word, bad);
  }

  /** Likely corrections for `word`, best first. */
  async suggest(word: string, limit = SUGGESTION_LIMIT): Promise<string[]> {
    await this.ready();
    if (!this.isActive()) return [];
    try {
      return await this.api.suggest(word, [...this.preferences!.languages], limit);
    } catch (error) {
      this.warn("suggest", "Spell check could not look up suggestions.", error);
      return [];
    }
  }

  /** The words the user has added to their own dictionary. */
  listWords(): Promise<string[]> {
    return this.api.listWords();
  }

  /** Adds a word to the user's dictionary. Throws if it cannot be saved. */
  async addWord(word: string): Promise<string[]> {
    const words = await this.api.addWord(word);
    this.invalidate();
    return words;
  }

  /** Takes a word out of the user's dictionary. Throws if that cannot be saved. */
  async removeWord(word: string): Promise<string[]> {
    const words = await this.api.removeWord(word);
    this.invalidate();
    return words;
  }

  /** Stops flagging `word` (in any capitalisation) until the app restarts. */
  ignore(word: string): void {
    this.ignored.add(word.toLowerCase());
    this.notify();
  }

  isIgnored(word: string): boolean {
    return this.ignored.has(word.toLowerCase());
  }

  /** Calls `listener` whenever what is flagged may have changed. */
  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private invalidate(): void {
    this.epoch += 1;
    this.verdicts.clear();
    this.notify();
  }

  private notify(): void {
    for (const listener of [...this.listeners]) {
      try {
        listener();
      } catch (error) {
        console.error("Spell check listener failed", error);
      }
    }
  }

  /** Logs a problem once per kind, so a broken backend does not flood the log. */
  private warn(kind: string, message: string, error: unknown): void {
    if (this.warned.has(kind)) return;
    this.warned.add(kind);
    console.warn(message, error);
  }
}

export const spellcheck = new SpellcheckService();
