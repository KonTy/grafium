import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  isMoreView,
  MOBILE_NAV_BAR_COUNT,
  MOBILE_NAV_DESTINATIONS,
  mobileBarDestinations,
  mobileMoreDestinations,
} from "./mobileNav";

const read = (path: string) => readFileSync(join(process.cwd(), path), "utf8");
const sidebar = read("src/components/Sidebar.svelte");
const app = read("src/App.svelte");

/** The sidebar's primary buttons, in the order they are drawn. */
function sidebarItems() {
  const start = sidebar.indexOf('<nav class="nav-items">');
  const nav = sidebar.slice(start, sidebar.indexOf("</nav>", start));
  return nav
    .split("\n")
    .filter((line) => line.trim().startsWith('<button class="nav-item"'))
    .map((line) => ({
      view: /currentView === "([\w-]+)"/.exec(line)?.[1],
      label: /aria-label="([^"]+)"/.exec(line)?.[1],
    }));
}

describe("phone navigation", () => {
  it("lists destinations in the desktop sidebar's order", () => {
    expect(sidebarItems()).toEqual(MOBILE_NAV_DESTINATIONS.map(({ view, label }) => ({ view, label })));
  });

  it("keeps the bar to what fits, in order, and continues the same list under More", () => {
    expect(MOBILE_NAV_BAR_COUNT).toBeLessThanOrEqual(5);
    expect(mobileBarDestinations.map((item) => item.barLabel ?? item.label)).toEqual([
      "Journal", "Tasks", "Library", "Studies", "Pages",
    ]);
    expect(mobileMoreDestinations.map((item) => item.label)).toEqual([
      "Graph View", "Flashcards", "Chat", "Jobs", "Settings",
    ]);
    expect([...mobileBarDestinations, ...mobileMoreDestinations]).toEqual([...MOBILE_NAV_DESTINATIONS]);
  });

  it("never lists a destination twice", () => {
    for (const key of ["view", "target", "label"] as const) {
      const values = MOBILE_NAV_DESTINATIONS.map((item) => item[key]);
      expect(new Set(values).size).toBe(values.length);
    }
  });

  it("marks More as current only for views that live behind it", () => {
    expect(isMoreView("settings")).toBe(true);
    expect(isMoreView("jobs")).toBe(true);
    expect(isMoreView("graph")).toBe(true);
    expect(isMoreView("journal")).toBe(false);
    expect(isMoreView("page")).toBe(false);
  });

  it("only offers targets the app knows how to open", () => {
    for (const { target } of MOBILE_NAV_DESTINATIONS) {
      expect(app).toContain(`"${target}"`);
    }
  });
});
