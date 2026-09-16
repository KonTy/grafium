import { describe, expect, it } from "vitest";
import { applySettingsSearch } from "./settingsSearch";

function fixture(): HTMLDivElement {
  const root = document.createElement("div");
  root.innerHTML = `
    <details class="settings-section" open>
      <summary><span class="section-title">General</span></summary>
      <div class="setting-row">Block hierarchy guide lines</div>
      <div class="setting-row">Re-index Graph</div>
    </details>
    <details class="settings-section">
      <summary><span class="section-title">Theme</span></summary>
      <button class="theme-card">Catppuccin</button>
      <button class="theme-card">Matrix</button>
    </details>
    <details class="settings-section">
      <summary><span class="section-title">Keyboard Shortcuts</span></summary>
      <div class="keymap-row">Go to Chat tab Alt-C</div>
      <div class="keymap-row">Go to today's journal g j</div>
    </details>
  `;
  return root;
}

describe("applySettingsSearch", () => {
  it("shows every section and row when the query is empty", () => {
    const root = fixture();
    const result = applySettingsSearch(root, "   ");
    expect(result.sections).toBe(3);
    expect(root.querySelectorAll("details.settings-section:not([hidden])")).toHaveLength(3);
    expect(root.querySelectorAll(".setting-row:not([hidden]), .theme-card:not([hidden]), .keymap-row:not([hidden])")).toHaveLength(6);
  });

  it("filters to settings that actually contain the query", () => {
    const root = fixture();
    applySettingsSearch(root, "chat");
    const visible = [...root.querySelectorAll<HTMLDetailsElement>("details.settings-section")].filter((el) => !el.hidden);
    expect(visible).toHaveLength(1);
    expect(visible[0].querySelector(".section-title")?.textContent).toBe("Keyboard Shortcuts");
    expect(visible[0].open).toBe(true);
    const rows = [...visible[0].querySelectorAll<HTMLElement>(".keymap-row")];
    expect(rows.filter((row) => !row.hidden).map((row) => row.textContent?.trim())).toEqual([
      "Go to Chat tab Alt-C",
    ]);
  });

  it("opens a whole section when the section title matches", () => {
    const root = fixture();
    applySettingsSearch(root, "theme");
    const theme = [...root.querySelectorAll("details.settings-section")].find(
      (el) => el.querySelector(".section-title")?.textContent === "Theme",
    ) as HTMLDetailsElement;
    expect(theme.hidden).toBe(false);
    expect(theme.open).toBe(true);
    expect([...theme.querySelectorAll<HTMLElement>(".theme-card")].every((card) => !card.hidden)).toBe(true);
  });

  it("does not assemble chat from scattered letters or different rows", () => {
    const root = fixture();
    root.insertAdjacentHTML("beforeend", `
      <details class="settings-section">
        <summary><span class="section-title">Unrelated</span></summary>
        <div class="setting-row">Cache heavy assets</div>
        <div class="setting-row">Colour</div>
        <div class="setting-row">Height</div>
        <div class="setting-row">Appearance</div>
        <div class="setting-row">Timing</div>
      </details>`);
    expect(applySettingsSearch(root, "chat").sections).toBe(1);
    expect(root.lastElementChild?.hasAttribute("hidden")).toBe(true);
  });

  it("matches literal word fragments in any order within the row and its headings", () => {
    const root = fixture();
    expect(applySettingsSearch(root, "  SHORTCUTS   chAT ").sections).toBe(1);
    const rows = [...root.querySelectorAll<HTMLElement>(".keymap-row")];
    expect(rows.filter((row) => !row.hidden)).toHaveLength(1);
    expect(applySettingsSearch(root, "block guide").sections).toBe(1);
    expect(applySettingsSearch(root, "block matrix").sections).toBe(0);
  });

  it("matches standalone help without revealing the rest of its section", () => {
    const root = fixture();
    root.insertAdjacentHTML("beforeend", `
      <details class="settings-section">
        <summary><span class="section-title">Research</span></summary>
        <p class="field-hint">This is the workflow used by Chat.</p>
        <div class="field-group">Max rounds</div>
        <div class="field-group">Max sources</div>
      </details>`);
    applySettingsSearch(root, "chat");
    const section = root.lastElementChild as HTMLElement;
    expect(section.hidden).toBe(false);
    expect(section.querySelector<HTMLElement>(".field-hint")?.hidden).toBe(false);
    expect([...section.querySelectorAll<HTMLElement>(".field-group")].every((row) => row.hidden)).toBe(true);
  });

  it("ignores prompt contents and dropdown options instead of finding invisible text", () => {
    const root = fixture();
    root.insertAdjacentHTML("beforeend", `
      <details class="settings-section">
        <summary><span class="section-title">AI</span></summary>
        <div class="field-group">Model <select><option>Chat model</option></select></div>
        <div class="field-group">Prompt <textarea>Hidden chat instructions</textarea></div>
        <div class="field-group">API key <input type="password" value="chat" /></div>
      </details>`);
    applySettingsSearch(root, "chat");
    expect(root.lastElementChild?.hasAttribute("hidden")).toBe(true);
  });

  it("keeps subsection context and required save controls without showing unrelated groups", () => {
    const root = document.createElement("div");
    root.innerHTML = `
      <details class="settings-section">
        <summary><span class="section-title">AI</span></summary>
        <div class="ai-settings">
          <div class="settings-section" id="local">
            <h4>Local provider</h4><div class="field-group">Base URL <input value="unchanged" /></div>
          </div>
          <div class="settings-section" id="whisper">
            <h4>Whisper transcription</h4>
            <div class="field-group">Language <input value="en" /></div>
            <div class="field-group">Model <input value="base" /></div>
            <div class="actions-section"><button>Save</button></div>
          </div>
        </div>
      </details>`;
    applySettingsSearch(root, "whisper lang");
    expect(root.querySelector<HTMLElement>("#local")?.hidden).toBe(true);
    expect(root.querySelector<HTMLElement>("#whisper")?.hidden).toBe(false);
    expect(root.querySelector<HTMLElement>(".actions-section")?.hidden).toBe(false);
    expect(root.querySelector<HTMLInputElement>("#whisper input")?.value).toBe("en");
    applySettingsSearch(root, "");
    expect(root.querySelectorAll("[hidden]")).toHaveLength(0);
  });

  it("hides empty shortcut categories and restores them when clearing", () => {
    const root = document.createElement("div");
    root.innerHTML = `
      <details class="settings-section">
        <summary><span class="section-title">Keyboard Shortcuts</span></summary>
        <div class="keymap-category"><h3>Navigation</h3><div class="keymap-row">Go to Chat</div></div>
        <div class="keymap-category"><h3>Editing</h3><div class="keymap-row">Indent block</div></div>
      </details>`;
    applySettingsSearch(root, "chat");
    const groups = [...root.querySelectorAll<HTMLElement>(".keymap-category")];
    expect(groups.map((group) => group.hidden)).toEqual([false, true]);
    expect(applySettingsSearch(root, "zzzz").sections).toBe(0);
    applySettingsSearch(root, "");
    expect(root.querySelectorAll("[hidden]")).toHaveLength(0);
  });
});
