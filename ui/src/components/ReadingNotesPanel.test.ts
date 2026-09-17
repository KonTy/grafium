import { describe, expect, it } from "vitest";
import panel from "./ReadingNotesPanel.svelte?raw";
import referencePanel from "./ReferencePanel.svelte?raw";
import pageContent from "./PageContent.svelte?raw";
import tools from "./PageAssistantTools.svelte?raw";

describe("Notes panel integration", () => {
  it("mounts outside the AI configuration gate and follows the existing journal-day source", () => {
    const notesBranch = referencePanel.indexOf('{#if activeTab === "notes"}');
    expect(notesBranch).toBeGreaterThan(0);
    expect(referencePanel).not.toContain("health?.enabled");
    expect(referencePanel).toContain('<ReadingNotesPanel pageId={sourcePageId}');
    expect(referencePanel).toContain('let activeTab = $state<"chat" | "notes"');
    expect(panel).not.toContain("aiHealthCheck");
    expect(panel).not.toContain("aiGenerate");
    expect(panel).toContain("const captured = untrack(() => get(readingSelection))");
    expect(referencePanel).toContain("initialNoteLabel={noteFocusLabel} initialNotePageId={noteFocusPageId} {noteFocusTrigger}");
    expect(panel).toContain("await loaded");
    expect(panel).toContain("applyReadingNoteFocusRequest(destination, id, requestedLabel, requestedTrigger)");
  });

  it("keeps the stable accessible controls and honest Markdown/draft storage copy", () => {
    for (const control of ['aria-label="Reading note"', 'aria-label="Notes scope"', "Save note", "New note", "Edit note", "Use selection"]) {
      expect(panel).toContain(control);
    }
    expect(panel).toContain('<option value="page"');
    expect(panel).toContain('<option value="all"');
    expect(panel).toContain('class="reading-notes-panel"');
    expect(panel).toContain('class="reading-note-card"');
    expect(panel).toContain("New notes are footnotes in the source Markdown file. Save notes before closing the app.");
    expect(panel).toContain("Unsaved draft");
    expect(panel).toContain("not after closing the app");
    expect(panel).toContain("view.draft.note.filePath");
    expect(panel).toContain("Review changed saved version");
    expect(panel).toContain("Use reviewed version for retry");
  });

  it("uses read-only quote previews and explicit ID-based source/note navigation", () => {
    expect(panel).toContain("{note.quote}</blockquote>");
    expect(panel).toContain("renderReadingNoteBody(note, view.graphPath)");
    expect(panel).toContain("onNavigate({ id: note.source.pageId })");
    expect(panel).toContain("onNavigate({ id: note.notePageId })");
    expect(panel).toContain('new CustomEvent("navigate-page", { detail: target })');
    expect(panel).not.toContain("onNavigate(note.source.pageTitle)");
    expect(panel).not.toContain("createPage(");
  });

  it("keeps the full Notes surface scrollable and tabs wrapped at narrow widths", () => {
    expect(panel).toMatch(/\.reading-notes-panel \{[^}]*min-height: 0;[^}]*overflow-y: auto/);
    expect(panel).toContain(".reading-notes-panel > * { flex-shrink: 0; }");
    expect(referencePanel).toMatch(/\.panel-tabs \{[^}]*flex-wrap: wrap/);
    expect(referencePanel).toMatch(/\.panel-content \{[^}]*min-height: 0/);
  });

  it("filters legacy string-only AI collectors without narrowing their concurrency snapshots", () => {
    expect(pageContent).toContain("readingSourceBlocks(blocks).filter((b) => selectedBlockIds.has(b.id))");
    expect(tools).toContain("readingSourceBlocks(source.snapshot)");
    expect(tools).toContain("expectedBlocks: source.snapshot");
  });
});
