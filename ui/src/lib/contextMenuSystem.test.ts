import { readdirSync, readFileSync } from "node:fs";
import { join, relative, sep } from "node:path";
import { describe, expect, it } from "vitest";

const SRC = join(process.cwd(), "src");

function sources(dir = SRC): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sources(path);
    const isSource = /\.(svelte|ts)$/.test(entry.name) && !/\.(test|spec|d)\.ts$/.test(entry.name);
    return isSource ? [path] : [];
  });
}

const files = sources().map((path) => ({
  name: relative(SRC, path).split(sep).join("/"),
  text: readFileSync(path, "utf8"),
}));

function filesMatching(pattern: RegExp, only: RegExp = /./): string[] {
  return files.filter(({ name, text }) => only.test(name) && pattern.test(text)).map(({ name }) => name);
}

// The app has one context menu: components hand it sections (`use:contextMenu`)
// and it decides how to draw them. A second implementation would bring back the
// menus that opened in the wrong place and the webview's own menu on top.
describe("the app's single context menu", () => {
  it("is the only code that listens for a contextmenu event", () => {
    expect(filesMatching(/oncontextmenu|on:contextmenu|["']contextmenu["']|\bcontextmenu\s*:/)).toEqual([
      "components/ContextMenuView.svelte",
      "lib/contextMenuHost.svelte.ts",
    ]);
  });

  it("is the only component that draws the menu surface", () => {
    expect(filesMatching(/app-context-menu/)).toEqual(["components/ContextMenuView.svelte", "lib/mainPaneScroll.ts"]);
  });

  it("leaves role=menu to it and to the dropdowns that open from a button", () => {
    expect(filesMatching(/role=["']menu["']/, /\.svelte$/).sort()).toEqual([
      "components/AppMenu.svelte",
      "components/ContextMenuView.svelte",
      "components/GraphMenu.svelte",
      "components/MobileNav.svelte",
      "components/PageMenu.svelte",
    ]);
  });

  it("is mounted once, and listening, by the app", () => {
    const app = files.find(({ name }) => name === "App.svelte")!.text;
    expect(app.match(/<ContextMenuHost\s*\/>/g)).toHaveLength(1);
    expect(app).toMatch(/onMount\(\(\) => installContextMenu\(\)\)/);
  });

  it("has no copy of the menus it replaced", () => {
    expect(
      filesMatching(/selection-context-menu|image-size-menu|make-link-menu|page-action-menu|page-action-item|SpellcheckMenu/),
    ).toEqual([]);
  });
});
