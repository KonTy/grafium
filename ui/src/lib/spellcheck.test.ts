import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SpellcheckPreferences } from "./api";
import { SpellcheckService, type SpellApi } from "./spellcheck";

const ENGLISH: SpellcheckPreferences = { enabled: true, languages: ["en"] };

/** A fake backend that calls any word starting with "x" misspelled. */
function fakeApi(overrides: Partial<SpellApi> = {}): SpellApi & { [K in keyof SpellApi]: ReturnType<typeof vi.fn> } {
  const api = {
    getPreferences: vi.fn(async () => ({ ...ENGLISH })),
    savePreferences: vi.fn(async (patch: Partial<SpellcheckPreferences>) => ({ ...ENGLISH, ...patch })),
    check: vi.fn(async (words: string[]) => words.map((word) => word.startsWith("x"))),
    suggest: vi.fn(async (word: string) => [`${word}-fixed`]),
    listWords: vi.fn(async () => ["zork"]),
    addWord: vi.fn(async (word: string) => [word]),
    removeWord: vi.fn(async () => []),
    ...overrides,
  };
  return api as never;
}

describe("SpellcheckService", () => {
  let warn: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    warn = vi.spyOn(console, "warn").mockImplementation(() => {});
  });

  afterEach(() => {
    warn.mockRestore();
  });

  it("reads the settings once, passing the UI locale", async () => {
    const api = fakeApi();
    const service = new SpellcheckService(api, () => "es-MX");
    await Promise.all([service.ready(), service.ready()]);
    await service.ready();
    expect(api.getPreferences).toHaveBeenCalledTimes(1);
    expect(api.getPreferences).toHaveBeenCalledWith("es-MX");
    expect(service.isActive()).toBe(true);
    expect(service.current()).toEqual(ENGLISH);
  });

  it("reports the misspelled words, each looked up once", async () => {
    const api = fakeApi();
    const service = new SpellcheckService(api);
    const bad = await service.check(["xray", "fine", "xray", "also"]);
    expect([...bad]).toEqual(["xray"]);
    expect(api.check).toHaveBeenCalledTimes(1);
    expect(api.check).toHaveBeenCalledWith(["xray", "fine", "also"], ["en"]);
  });

  it("remembers verdicts so repeat checks stay in the page", async () => {
    const api = fakeApi();
    const service = new SpellcheckService(api);
    await service.check(["xray", "fine"]);
    const bad = await service.check(["fine", "xray", "newword"]);
    expect([...bad]).toEqual(["xray"]);
    expect(api.check).toHaveBeenCalledTimes(2);
    expect(api.check).toHaveBeenLastCalledWith(["newword"], ["en"]);
  });

  it("sends big pages in batches", async () => {
    const api = fakeApi();
    const service = new SpellcheckService(api);
    const words = Array.from({ length: 1201 }, (_, i) => `word${i}`);
    await service.check(words);
    expect(api.check.mock.calls.map(([batch]) => (batch as string[]).length)).toEqual([500, 500, 201]);
  });

  it("checks nothing while it is off or has no language", async () => {
    for (const prefs of [
      { enabled: false, languages: ["en"] },
      { enabled: true, languages: [] },
    ]) {
      const api = fakeApi({ getPreferences: vi.fn(async () => prefs) });
      const service = new SpellcheckService(api);
      expect(await service.check(["xray"])).toEqual(new Set());
      expect(await service.suggest("xray")).toEqual([]);
      expect(service.isActive()).toBe(false);
      expect(api.check).not.toHaveBeenCalled();
      expect(api.suggest).not.toHaveBeenCalled();
    }
  });

  it("stays off, tells why, and writes nothing when the settings cannot be read", async () => {
    let broken = true;
    const api = fakeApi({
      getPreferences: vi.fn(async () => {
        if (broken) throw new Error("spellcheck.json is not valid JSON");
        return { ...ENGLISH };
      }),
    });
    const service = new SpellcheckService(api);
    expect(await service.check(["xray"])).toEqual(new Set());
    expect(service.isActive()).toBe(false);
    expect(service.current()).toBeNull();
    expect(service.loadError()).toBe("spellcheck.json is not valid JSON");
    expect(warn).toHaveBeenCalledTimes(1);
    expect(api.savePreferences).not.toHaveBeenCalled();

    broken = false;
    const changed = vi.fn();
    service.subscribe(changed);
    await service.reload();
    expect(service.loadError()).toBeNull();
    expect(service.isActive()).toBe(true);
    expect(changed).toHaveBeenCalledTimes(1);
    expect([...(await service.check(["xray"]))]).toEqual(["xray"]);
  });

  it("saves new settings, forgets old verdicts and tells subscribers", async () => {
    const api = fakeApi();
    const service = new SpellcheckService(api, () => "fr");
    const changed = vi.fn();
    service.subscribe(changed);
    await service.check(["xray"]);

    const saved = await service.setPreferences({ languages: ["en", "es"] });
    expect(api.savePreferences).toHaveBeenCalledWith({ languages: ["en", "es"] }, "fr");
    expect(saved).toEqual({ enabled: true, languages: ["en", "es"] });
    expect(service.current()).toEqual(saved);
    expect(changed).toHaveBeenCalledTimes(1);

    await service.check(["xray"]);
    expect(api.check).toHaveBeenCalledTimes(2);
    expect(api.check).toHaveBeenLastCalledWith(["xray"], ["en", "es"]);
  });

  it("keeps its settings and stays quiet when saving fails", async () => {
    const api = fakeApi({
      savePreferences: vi.fn(async () => {
        throw new Error("disk full");
      }),
    });
    const service = new SpellcheckService(api);
    const changed = vi.fn();
    service.subscribe(changed);
    await expect(service.setPreferences({ enabled: false })).rejects.toThrow("disk full");
    expect(service.current()).toEqual(ENGLISH);
    expect(changed).not.toHaveBeenCalled();
  });

  it("does not hand out its internal settings", async () => {
    const service = new SpellcheckService(fakeApi());
    await service.ready();
    service.current()!.languages.push("fr");
    expect(service.current()).toEqual(ENGLISH);
  });

  it("does not keep verdicts reached under settings that changed mid-check", async () => {
    let release!: () => void;
    const gate = new Promise<void>((resolve) => (release = resolve));
    const api = fakeApi({
      check: vi.fn(async (words: string[]) => {
        await gate;
        return words.map(() => true);
      }),
    });
    const service = new SpellcheckService(api);
    const pending = service.check(["xray"]);
    await vi.waitFor(() => expect(api.check).toHaveBeenCalledTimes(1));
    await service.setPreferences({ languages: ["fr"] });
    release();
    await pending;

    await service.check(["xray"]);
    expect(api.check).toHaveBeenCalledTimes(2);
  });

  it("treats words as correct, and complains once, when the backend fails", async () => {
    const api = fakeApi();
    const service = new SpellcheckService(api);
    expect([...(await service.check(["xray"]))]).toEqual(["xray"]);

    api.check.mockRejectedValue(new Error("boom"));
    const bad = await service.check(["xray", "xylophone", "other"]);
    expect([...bad]).toEqual(["xray"]);
    await service.check(["another"]);
    expect(warn).toHaveBeenCalledTimes(1);

    api.check.mockResolvedValue([false]);
    expect([...(await service.check(["other"]))]).toEqual([]);
  });

  it("asks for suggestions in the enabled languages and survives errors", async () => {
    const api = fakeApi();
    const service = new SpellcheckService(api);
    expect(await service.suggest("wrold")).toEqual(["wrold-fixed"]);
    expect(api.suggest).toHaveBeenCalledWith("wrold", ["en"], 5);
    await service.suggest("wrold", 2);
    expect(api.suggest).toHaveBeenLastCalledWith("wrold", ["en"], 2);

    api.suggest.mockRejectedValue(new Error("boom"));
    expect(await service.suggest("wrold")).toEqual([]);
  });

  it("re-checks after the user's dictionary changes", async () => {
    const api = fakeApi();
    const service = new SpellcheckService(api);
    const changed = vi.fn();
    service.subscribe(changed);
    await service.check(["xray"]);

    expect(await service.addWord("xray")).toEqual(["xray"]);
    expect(api.addWord).toHaveBeenCalledWith("xray");
    expect(changed).toHaveBeenCalledTimes(1);
    await service.check(["xray"]);
    expect(api.check).toHaveBeenCalledTimes(2);

    await service.removeWord("xray");
    expect(changed).toHaveBeenCalledTimes(2);
    expect(await service.listWords()).toEqual(["zork"]);
  });

  it("does not announce a dictionary change that failed", async () => {
    const api = fakeApi({
      addWord: vi.fn(async () => {
        throw new Error("read-only");
      }),
    });
    const service = new SpellcheckService(api);
    const changed = vi.fn();
    service.subscribe(changed);
    await expect(service.addWord("xray")).rejects.toThrow("read-only");
    expect(changed).not.toHaveBeenCalled();
  });

  it("ignores a word for the session in any capitalisation, without re-checking", async () => {
    const api = fakeApi();
    const service = new SpellcheckService(api);
    const changed = vi.fn();
    service.subscribe(changed);
    await service.check(["xray"]);

    expect(service.isIgnored("Xray")).toBe(false);
    service.ignore("xRay");
    expect(service.isIgnored("XRAY")).toBe(true);
    expect(changed).toHaveBeenCalledTimes(1);
    await service.check(["xray"]);
    expect(api.check).toHaveBeenCalledTimes(1);
  });

  it("stops notifying after unsubscribe and survives a failing listener", async () => {
    const service = new SpellcheckService(fakeApi());
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const second = vi.fn();
    service.subscribe(() => {
      throw new Error("listener bug");
    });
    const stop = service.subscribe(second);
    service.ignore("a");
    expect(second).toHaveBeenCalledTimes(1);
    stop();
    service.ignore("b");
    expect(second).toHaveBeenCalledTimes(1);
    error.mockRestore();
  });
});
