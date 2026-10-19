import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SpellcheckSettings from "./SpellcheckSettings.svelte";
import { listSpellLanguages } from "../lib/api";
import type { SpellLanguage, SpellcheckPreferences } from "../lib/api";
import { spellcheck } from "../lib/spellcheck";
import { applySettingsSearch } from "../lib/settingsSearch";

vi.mock("../lib/api", () => ({ listSpellLanguages: vi.fn() }));
vi.mock("../lib/spellcheck", () => ({
  spellcheck: {
    ready: vi.fn(),
    current: vi.fn(),
    loadError: vi.fn(),
    reload: vi.fn(),
    setPreferences: vi.fn(),
    listWords: vi.fn(),
    addWord: vi.fn(),
    removeWord: vi.fn(),
  },
}));

const LANGUAGES: SpellLanguage[] = [
  { code: "en", name: "English (US)", nativeName: "English (US)", script: "latin", memoryMb: 3 },
  { code: "es", name: "Spanish", nativeName: "Español", script: "latin", memoryMb: 6 },
  { code: "ar", name: "Arabic", nativeName: "العربية", script: "arabic", memoryMb: 45 },
] as SpellLanguage[];

let host: HTMLDivElement;
let component: ReturnType<typeof mount> | null = null;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  vi.mocked(listSpellLanguages).mockResolvedValue(LANGUAGES);
  vi.mocked(spellcheck.ready).mockResolvedValue();
  vi.mocked(spellcheck.current).mockReturnValue({ enabled: true, languages: ["en", "es"] });
  vi.mocked(spellcheck.loadError).mockReturnValue(null);
  vi.mocked(spellcheck.listWords).mockResolvedValue(["kubernetes", "Grafium"]);
});

afterEach(async () => {
  if (component) await unmount(component);
  component = null;
  host.remove();
  vi.resetAllMocks();
});

async function open(target: HTMLElement = host) {
  component = mount(SpellcheckSettings, { target });
  flushSync();
  await vi.waitFor(() => expect(target.querySelector(".language-list label")).not.toBeNull());
  flushSync();
}

const box = (label: string) => {
  const found = [...host.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')].find(
    (input) => input.closest("label")?.textContent?.includes(label),
  );
  expect(found, `checkbox: ${label}`).toBeDefined();
  return found!;
};
const alert = () => host.querySelector('[role="alert"]')?.textContent?.replace(/\s+/g, " ").trim();
const words = () => [...host.querySelectorAll(".words .word")].map((word) => word.textContent);
const text = () => host.textContent?.replace(/\s+/g, " ");

async function until(check: () => void) {
  await vi.waitFor(check);
  flushSync();
}

function type(value: string) {
  const input = host.querySelector<HTMLInputElement>('input[type="text"]')!;
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
  return input;
}

describe("the languages", () => {
  it("are listed with their memory, and the saved choice is ticked", async () => {
    await open();
    expect(text()).toContain("English (US) ~3 MB");
    expect(text()).toContain("Español Spanish · ~6 MB");
    expect(text()).toContain("العربية Arabic · ~45 MB");
    expect(box("English (US)").checked).toBe(true);
    expect(box("Español").checked).toBe(true);
    expect(box("العربية").checked).toBe(false);
    expect(box("Underline misspelled words").checked).toBe(true);
  });

  it("are saved in the order they are listed in", async () => {
    vi.mocked(spellcheck.current).mockReturnValue({ enabled: true, languages: ["es"] });
    vi.mocked(spellcheck.setPreferences).mockResolvedValue({ enabled: true, languages: ["en", "es"] });
    await open();

    box("English (US)").click();
    await until(() =>
      expect(spellcheck.setPreferences).toHaveBeenCalledWith({ languages: ["en", "es"] }),
    );
    expect(box("English (US)").checked).toBe(true);
  });

  it("can be unticked", async () => {
    vi.mocked(spellcheck.setPreferences).mockResolvedValue({ enabled: true, languages: ["en"] });
    await open();

    box("Español").click();
    await until(() =>
      expect(spellcheck.setPreferences).toHaveBeenCalledWith({ languages: ["en"] }),
    );
    expect(box("Español").checked).toBe(false);
  });

  it("are greyed out while spell check is off", async () => {
    const off: SpellcheckPreferences = { enabled: false, languages: ["en", "es"] };
    vi.mocked(spellcheck.setPreferences).mockResolvedValue(off);
    await open();
    expect(box("Español").disabled).toBe(false);

    box("Underline misspelled words").click();
    await until(() => expect(spellcheck.setPreferences).toHaveBeenCalledWith({ enabled: false }));
    await until(() => expect(box("Español").disabled).toBe(true));
    expect(box("Underline misspelled words").checked).toBe(false);
  });

  it("warn when none is chosen", async () => {
    vi.mocked(spellcheck.current).mockReturnValue({ enabled: true, languages: [] });
    await open();
    expect(text()).toContain("No language is selected, so nothing is checked.");
  });

  it("put the box back and say why when a change could not be saved", async () => {
    vi.mocked(spellcheck.setPreferences).mockRejectedValue("disk full");
    await open();

    box("Español").click();
    expect(box("Español").checked).toBe(false);
    await until(() => expect(alert()).toBe("Could not save the spell check settings: disk full"));
    await until(() => expect(box("Español").checked).toBe(true));

    box("Underline misspelled words").click();
    expect(box("Underline misspelled words").checked).toBe(false);
    await until(() => expect(spellcheck.setPreferences).toHaveBeenCalledTimes(2));
    await until(() => expect(box("Underline misspelled words").checked).toBe(true));
  });
});

describe("my dictionary", () => {
  it("lists the words", async () => {
    await open();
    expect(words()).toEqual(["kubernetes", "Grafium"]);
  });

  it("says so when there are none", async () => {
    vi.mocked(spellcheck.listWords).mockResolvedValue([]);
    await open();
    expect(text()).toContain("No words added yet.");
  });

  it("adds a word without its surrounding spaces and clears the box", async () => {
    vi.mocked(spellcheck.addWord).mockResolvedValue(["Grafium", "kubernetes", "terraform"]);
    await open();

    const input = type("  terraform ");
    host.querySelector<HTMLFormElement>("form")!.requestSubmit();
    await until(() => expect(words()).toEqual(["Grafium", "kubernetes", "terraform"]));
    expect(spellcheck.addWord).toHaveBeenCalledWith("terraform");
    expect(input.value).toBe("");
  });

  it("will not submit an empty box", async () => {
    await open();
    const add = [...host.querySelectorAll<HTMLButtonElement>("form button")][0];
    expect(add.disabled).toBe(true);
    type("   ");
    expect(add.disabled).toBe(true);
    host.querySelector<HTMLFormElement>("form")!.requestSubmit();
    expect(spellcheck.addWord).not.toHaveBeenCalled();
  });

  it("keeps what was typed and says why a word was refused", async () => {
    vi.mocked(spellcheck.addWord).mockRejectedValue("“a b” is not a word that can be added to the dictionary");
    await open();

    const input = type("a b");
    host.querySelector<HTMLFormElement>("form")!.requestSubmit();
    await until(() =>
      expect(alert()).toBe(
        "Could not add the word: “a b” is not a word that can be added to the dictionary",
      ),
    );
    expect(input.value).toBe("a b");
    expect(words()).toEqual(["kubernetes", "Grafium"]);
  });

  it("removes a word", async () => {
    vi.mocked(spellcheck.removeWord).mockResolvedValue(["Grafium"]);
    await open();

    host
      .querySelector<HTMLButtonElement>('button[aria-label="Remove kubernetes from the dictionary"]')!
      .click();
    await until(() => expect(words()).toEqual(["Grafium"]));
    expect(spellcheck.removeWord).toHaveBeenCalledWith("kubernetes");
  });

  it("says why a word could not be removed, and keeps it listed", async () => {
    vi.mocked(spellcheck.removeWord).mockRejectedValue(new Error("read-only"));
    await open();

    host
      .querySelector<HTMLButtonElement>('button[aria-label="Remove Grafium from the dictionary"]')!
      .click();
    await until(() => expect(alert()).toBe("Could not remove the word: read-only"));
    expect(words()).toEqual(["kubernetes", "Grafium"]);
  });
});

describe("when the settings cannot be read", () => {
  it("says so, keeps the controls off, and starts again on request", async () => {
    vi.mocked(spellcheck.current).mockReturnValue(null);
    vi.mocked(spellcheck.loadError).mockReturnValue("invalid JSON");
    await open();

    expect(alert()).toBe(
      "Spell check is off because its settings could not be read: invalid JSON Try again",
    );
    expect(box("Underline misspelled words").disabled).toBe(true);
    expect(box("English (US)").disabled).toBe(true);

    vi.mocked(spellcheck.reload).mockImplementation(async () => {
      vi.mocked(spellcheck.current).mockReturnValue({ enabled: true, languages: ["en"] });
      vi.mocked(spellcheck.loadError).mockReturnValue(null);
    });
    [...host.querySelectorAll("button")].find((b) => b.textContent === "Try again")!.click();
    await until(() => expect(host.querySelector('[role="alert"]')).toBeNull());
    expect(spellcheck.reload).toHaveBeenCalledOnce();
    expect(box("Underline misspelled words").disabled).toBe(false);
    expect(box("English (US)").checked).toBe(true);
  });

  it("still lists the dictionaries when only the personal words fail to load", async () => {
    vi.mocked(spellcheck.listWords).mockRejectedValue("no access");
    await open();
    expect(alert()).toBe("Could not load the spell check lists: no access");
    expect(text()).toContain("Español");
  });
});

describe("the settings search", () => {
  function inSection() {
    const root = document.createElement("div");
    root.innerHTML = `<details class="settings-section"><summary><span class="section-title">Spell check</span></summary><div class="section-content"></div></details>`;
    host.append(root);
    return { root, content: root.querySelector<HTMLElement>(".section-content")! };
  }
  const visibleRows = (root: HTMLElement) =>
    [...root.querySelectorAll<HTMLElement>(".setting-row")].filter((row) => !row.hidden);

  it("finds a language, the help text and a saved word, and hides the rest", async () => {
    const { root, content } = inSection();
    await open(content);
    const shown = () =>
      visibleRows(root).map((row) => row.querySelector(".setting-label")?.textContent);

    expect(applySettingsSearch(root, "Español").sections).toBe(1);
    expect(shown()).toEqual(["Languages"]);

    applySettingsSearch(root, "offline");
    expect(shown()).toEqual(["Check spelling"]);

    applySettingsSearch(root, "kubernetes");
    expect(shown()).toEqual(["My dictionary"]);

    applySettingsSearch(root, "spell");
    expect(shown()).toHaveLength(3);
    applySettingsSearch(root, "");
    expect(shown()).toHaveLength(3);
  });
});
