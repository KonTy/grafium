import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const app = readFileSync(join(process.cwd(), "src/App.svelte"), "utf8");
const titleBar = readFileSync(join(process.cwd(), "src/components/TitleBar.svelte"), "utf8");
const journal = readFileSync(join(process.cwd(), "src/components/JournalView.svelte"), "utf8");
const jobs = readFileSync(join(process.cwd(), "src/components/JobActivity.svelte"), "utf8");

describe("compact title bar actions", () => {
  it("places journal navigation before Search on every platform, with a Zen fallback", () => {
    expect(app).toContain('showJournalActions={currentView === "journal"}');
    expect(app).toContain("showNavigationToolbar={zenMode}");

    const actions = titleBar.indexOf("{#if showJournalActions}");
    const search = titleBar.indexOf('shortcutTitle("Search", "search-global")');
    expect(actions).toBeGreaterThan(-1);
    expect(search).toBeGreaterThan(actions);
    expect(titleBar).toContain("data-journal-date-action");
    expect(titleBar).toContain('aria-label="Go to date"');
    expect(titleBar).toContain('aria-label="Go to link"');
    expect(journal).toContain("{#if showNavigationToolbar}");
  });

  it("uses registered shortcuts in hints without keeping the old Bionic toolbar button", () => {
    expect(titleBar).not.toContain("onToggleBionicReader");
    expect(titleBar).not.toContain("bionic-toggle-icon");
    expect(titleBar).toContain('shortcutTitle("Back", "go-backward")');
    expect(titleBar).toContain('shortcutTitle("Forward", "go-forward")');
    expect(titleBar).toContain('shortcutAria("toggle-right-sidebar")');
    for (const source of [titleBar, journal]) {
      expect(source).toContain('shortcutTitle("Go to date", "go-journal-date")');
      expect(source).toContain('shortcutTitle("Go to link", "go-link")');
      expect(source).not.toContain("Ctrl/Cmd+");
    }
  });

  it("registers reader actions and loads remembered text size at startup", () => {
    expect(app).toContain("toggleBionicReader,");
    expect(app).toContain("bookmark: () => { void bookmarkCurrentReading(); }");
    expect(app).toContain('new CustomEvent("grafium-bookmark", { cancelable: true })');
    expect(app).toContain("if (hasKeyboardOverlay(document)) return;");
    expect(app).toContain('currentView === "page" && currentPage && isOriginalBookPage(currentPage)');
    expect(app).toContain("loadReaderTextSizePreference();");
  });

  it("keeps the toolbar job count inside the title bar", () => {
    expect(jobs).toContain(".job-activity.toolbar .job-badge");
    expect(jobs).toContain("top: 1px;");
    expect(jobs).toContain("right: 1px;");
  });
});
