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
    const search = titleBar.indexOf('title="Search (Ctrl+K)"');
    expect(actions).toBeGreaterThan(-1);
    expect(search).toBeGreaterThan(actions);
    expect(titleBar).toContain("data-journal-date-action");
    expect(titleBar).toContain('aria-label="Go to date"');
    expect(titleBar).toContain('aria-label="Go to link"');
    expect(journal).toContain("{#if showNavigationToolbar}");
  });

  it("keeps the toolbar job count inside the title bar", () => {
    expect(jobs).toContain(".job-activity.toolbar .job-badge");
    expect(jobs).toContain("top: 1px;");
    expect(jobs).toContain("right: 1px;");
  });
});
