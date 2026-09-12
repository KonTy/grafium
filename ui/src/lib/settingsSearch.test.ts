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
      <div class="keymap-row">Go to Chat tab Ctrl-Shift-C</div>
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

  it("filters to settings whose text fuzzy-matches the query", () => {
    const root = fixture();
    applySettingsSearch(root, "chat");
    const visible = [...root.querySelectorAll("details.settings-section")].filter((el) => !el.hidden);
    expect(visible).toHaveLength(1);
    expect(visible[0].querySelector(".section-title")?.textContent).toBe("Keyboard Shortcuts");
    expect(visible[0].open).toBe(true);
    const rows = [...visible[0].querySelectorAll(".keymap-row")];
    expect(rows.filter((row) => !row.hidden).map((row) => row.textContent?.trim())).toEqual([
      "Go to Chat tab Ctrl-Shift-C",
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
    expect([...theme.querySelectorAll(".theme-card")].every((card) => !card.hidden)).toBe(true);
  });
});
