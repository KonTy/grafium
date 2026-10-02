import { describe, expect, it } from "vitest";
import app from "./App.svelte?raw";

describe("Library app integration", () => {
  it("routes local filter focus to the mounted Library shelf without stealing modal focus", () => {
    const focus = app.slice(app.indexOf("function focusLocalSearch()"), app.indexOf("function journalCursorTitle()"));
    expect(focus).toContain("if (hasKeyboardOverlay(document)) return false;");
    expect(focus).toContain('if (currentView === "library") return privateLibraryRef?.focusSearch() ?? false;');
    expect(app).toContain("<PrivateReaderLibrary bind:this={privateLibraryRef}");
  });

  it("opens the private shelf without querying the active graph", () => {
    const route = app.slice(app.indexOf('if (target === "__library__")'), app.indexOf('if (target === "__studies__")'));
    expect(route).toContain('currentView = "library"');
    expect(route).not.toContain("getGraphInfo");
    expect(route).toContain("restoreEntry.bookId");
    expect(app).toContain('library: "library"');
  });

  it("preserves the outgoing Library detail before clearing its selection", () => {
    const navigate = app.slice(app.indexOf("async function navigateToPage("), app.indexOf('// Handle special routes'));
    expect(navigate.indexOf("saveCurrentHistoryState(")).toBeLessThan(navigate.indexOf("privateBookId = null"));
    expect(navigate).toContain("if (navigationRequest !== studyNavigation) return");
    const library = app.slice(app.indexOf("async function openPrivateBook("), app.indexOf("async function addLibraryToStudies("));
    expect(library).toContain("const request = studyNavigation + 1");
    expect(library).toContain('request !== studyNavigation || currentView !== "library"');
  });

  it("records plan time without private playback state and routes explicit journal links", () => {
    expect(app).toContain('item.kind === "library" ? null : progress');
    expect(app).toContain("libraryBookmarkJournalSnippet(book, bookmark)");
    expect(app).toContain('window.addEventListener("click", handleLibraryLink, true)');
    expect(app).toContain("initialBookmarkId={privateBookmarkId}");
    expect(app).toContain("onJournalNote={(book, bookmark)");
  });
});
