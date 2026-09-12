import { fuzzyMatches } from "./fuzzy";

/** Leaf controls/rows inside a Settings section that can be filtered. */
export const SETTINGS_ITEM_SELECTOR = [
  ".setting-row",
  ".keymap-row",
  ".theme-card",
  ".sync-target-card",
  ".sync-add-btn",
  ".sync-add-form",
  ".sync-empty",
  ".sync-message",
  ".field-group",
  ".toggle-row",
  ".engine-item",
  ".about-info",
  ".about-details",
  ".section-desc",
  ".setting-desc",
  ".orphan-item",
  ".status-bar",
  ".actions-section",
  ".add-form",
  ".message",
  ".detail-row",
].join(",");

export interface SettingsSearchResult {
  sections: number;
  items: number;
}

function leafItems(section: HTMLElement): HTMLElement[] {
  const items = Array.from(section.querySelectorAll<HTMLElement>(SETTINGS_ITEM_SELECTOR));
  return items.filter((el) => !items.some((other) => other !== el && other.contains(el)));
}

/** Fuzzy-filter Settings sections and rows in place. Empty query shows everything. */
export function applySettingsSearch(root: HTMLElement, query: string): SettingsSearchResult {
  const sections = root.querySelectorAll<HTMLDetailsElement>(":scope > details.settings-section");
  const needle = query.trim();
  let visibleSections = 0;
  let visibleItems = 0;

  for (const section of sections) {
    const items = leafItems(section);
    if (!needle) {
      section.hidden = false;
      for (const el of items) el.hidden = false;
      visibleSections += 1;
      visibleItems += items.length;
      continue;
    }

    const title = section.querySelector(".section-title")?.textContent ?? "";
    const titleMatch = fuzzyMatches(title, needle);
    let anyItem = false;
    for (const el of items) {
      const match = titleMatch || fuzzyMatches(el.textContent || "", needle);
      el.hidden = !match;
      if (match) {
        anyItem = true;
        visibleItems += 1;
      }
    }

    const show = titleMatch || anyItem || fuzzyMatches(section.textContent || "", needle);
    section.hidden = !show;
    if (show) {
      section.open = true;
      visibleSections += 1;
      if (!anyItem && !titleMatch) {
        for (const el of items) el.hidden = false;
        visibleItems += items.length;
      }
    }
  }

  return { sections: visibleSections, items: visibleItems };
}
