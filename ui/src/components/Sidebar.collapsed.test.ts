import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const app = readFileSync(join(process.cwd(), "src/App.svelte"), "utf8");
const sidebar = readFileSync(join(process.cwd(), "src/components/Sidebar.svelte"), "utf8");
const titleBar = readFileSync(join(process.cwd(), "src/components/TitleBar.svelte"), "utf8");

describe("collapsed sidebar icon rail", () => {
  it("keeps the sidebar mounted outside Zen mode", () => {
    expect(app).toContain("{#if !zenMode}");
    expect(app).toContain("collapsed={!sidebarVisible}");
    expect(app).toContain("SIDEBAR_RAIL_WIDTH = 52");
    expect(app).toContain(".app-shell.zen .bottom-nav");
  });

  it("keeps primary navigation available as labelled icons", () => {
    expect(sidebar).toContain('<aside class="sidebar" class:collapsed');
    expect(sidebar).toContain('aria-label="Expand sidebar"');
    expect(sidebar).toContain('aria-label="Journal"');
    expect(sidebar).toContain('aria-label="All Pages"');
    expect(sidebar).toContain('aria-label="Settings"');
    expect(sidebar).toContain(".sidebar.collapsed .nav-item span");
  });

  it("aligns the title bar with both sidebar widths", () => {
    expect(titleBar).toContain("sidebarVisible ? sidebarWidth : 52");
    expect(titleBar).toContain("class:collapsed={!sidebarVisible}");
  });

  it("shows registered navigation hints but does not invent shortcuts for dynamic actions", () => {
    for (const [label, id] of [["Journal", "go-journal"], ["Library", "go-library"], ["Studies", "go-studies"],
      ["Tasks", "go-tasks"], ["All Pages", "go-all-pages"], ["Settings", "toggle-settings"]]) {
      expect(sidebar).toContain(`shortcutTitle("${label}", "${id}")`);
    }
    expect(sidebar).toContain('title="Create new page"');
    expect(sidebar).toContain('title="Jobs"');
    expect(sidebar).not.toContain('title="Expand sidebar (Ctrl+B)"');
  });

  it("focuses navigation without retaining the duplicate search UI", () => {
    expect(sidebar).toContain("export function focusNavigation()");
    expect(sidebar).toContain('".nav-item.active"');
    expect(sidebar).not.toContain("search-input");
    expect(sidebar).not.toContain("search-toggle");
    expect(sidebar).not.toContain("sidebarSearch");
    expect(app).toContain("sidebarRef?.focusNavigation()");
    expect(app).not.toContain("toggle-search");
    expect(sidebar).not.toContain("focusSearch");
    expect(app).not.toContain("sidebarRef?.focusSearch");
    expect(app).toContain("privateLibraryRef?.focusSearch()");
  });
});
