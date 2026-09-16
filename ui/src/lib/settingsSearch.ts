/** Logical controls/rows; nested labels and hints stay with their control. */
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
  ".section-desc",
  ".setting-desc",
  ".orphan-item",
  ".status-bar",
  ".actions-section",
  ".add-form",
  ".message",
  ".detail-row",
  ".field-hint",
  "button",
].join(",");

const SETTINGS_GROUP_SELECTOR = [
  ".keymap-category",
  ".ai-settings .settings-section",
  ".research-settings .settings-section",
  ".about-details",
].join(",");

export interface SettingsSearchResult {
  sections: number;
  items: number;
}

function topLevelItems(section: HTMLElement): HTMLElement[] {
  const items = Array.from(section.querySelectorAll<HTMLElement>(SETTINGS_ITEM_SELECTOR));
  return items.filter((el) => !items.some((other) => other !== el && other.contains(el)));
}

function searchText(element: Element): string {
  const walker = element.ownerDocument.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  const text: string[] = [];
  while (walker.nextNode()) {
    // Search labels/help, not long prompt values or invisible dropdown options.
    if (!walker.currentNode.parentElement?.closest("textarea, select, script, style")) {
      text.push(walker.currentNode.textContent ?? "");
    }
  }
  return text.join(" ").toLowerCase().replace(/\s+/g, " ").trim();
}

/** Match literal query terms within one row and its headings, never across unrelated rows. */
export function applySettingsSearch(root: HTMLElement, query: string): SettingsSearchResult {
  const sections = root.querySelectorAll<HTMLDetailsElement>(":scope > details.settings-section");
  const terms = query.toLowerCase().trim().split(/\s+/).filter(Boolean);
  const matches = (text: string) => terms.every((term) => text.includes(term));
  let visibleSections = 0;
  let visibleItems = 0;

  for (const section of sections) {
    const items = topLevelItems(section);
    const groups = Array.from(section.querySelectorAll<HTMLElement>(SETTINGS_GROUP_SELECTOR));
    if (!terms.length) {
      section.hidden = false;
      for (const group of groups) group.hidden = false;
      for (const el of items) el.hidden = false;
      visibleSections += 1;
      visibleItems += items.length;
      continue;
    }

    const heading = section.querySelector(".section-title");
    const title = heading ? searchText(heading) : "";
    const titleMatch = matches(title);
    const groupTitles = new Map(groups.map((group) => {
      const heading = group.querySelector(":scope > h3, :scope > h4");
      return [group, heading ? searchText(heading) : ""];
    }));
    for (const el of items) {
      const context = groups.filter((group) => group.contains(el)).map((group) => groupTitles.get(group));
      el.hidden = !matches([title, ...context, searchText(el)].join(" "));
    }

    // Keep the save/action row available when its settings contain a matching control.
    for (const actions of items.filter((item) => item.matches(".actions-section"))) {
      const container = actions.parentElement;
      if (container && items.some((item) => !item.hidden && container.contains(item)
        && item.querySelector("input, select, textarea"))) actions.hidden = false;
    }
    for (const group of groups) {
      group.hidden = !matches(`${title} ${groupTitles.get(group) ?? ""}`)
        && !items.some((item) => group.contains(item) && !item.hidden);
    }

    const matchingItems = items.filter((item) => !item.hidden);
    const show = titleMatch || matchingItems.length > 0 || groups.some((group) => !group.hidden);
    section.hidden = !show;
    if (show) {
      section.open = true;
      visibleSections += 1;
      visibleItems += matchingItems.length;
    }
  }

  return { sections: visibleSections, items: visibleItems };
}
