import { describe, expect, it } from "vitest";
import { helpPageTitle, isHelpContext, loadHelpPage, type HelpContext } from "./help";
import { vi } from "vitest";
import chatHelp from "../../src-tauri/resources/welcome/pages/Help - Chat.md?raw";
import aiHelp from "../../src-tauri/resources/welcome/pages/AI Setup And Privacy.md?raw";
import searchHelp from "../../src-tauri/resources/welcome/pages/Help - Search.md?raw";
import booksHelp from "../../src-tauri/resources/welcome/pages/Help - Books.md?raw";
import readerHelp from "../../src-tauri/resources/welcome/pages/Help - Private Reader.md?raw";
import studiesHelp from "../../src-tauri/resources/welcome/pages/Help - Studies.md?raw";
import sidebarSource from "../components/Sidebar.svelte?raw";
import editorHelp from "../../src-tauri/resources/welcome/pages/Help - Editor.md?raw";
import readingNotesPanel from "../components/ReadingNotesPanel.svelte?raw";
import settingsHelp from "../../src-tauri/resources/welcome/pages/Help - Settings.md?raw";
import syncHelp from "../../src-tauri/resources/welcome/pages/Help - Sync.md?raw";
import syncResolution from "../components/SyncConflictResolution.svelte?raw";
import helpIndex from "../../src-tauri/resources/welcome/pages/Grafium Help.md?raw";
import importDialog from "../components/BookImportDialog.svelte?raw";
import appSource from "../App.svelte?raw";
import recoverySource from "../components/RuntimeRecovery.svelte?raw";
import searchSource from "../components/GlobalSearchDialog.svelte?raw";

const api = vi.hoisted(() => ({ invoke: vi.fn().mockResolvedValue("AI help") }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: api.invoke }));

describe("contextual help", () => {
  it("routes private reading to its privacy and hardware guidance", async () => {
    expect(helpPageTitle("reader")).toBe("Help - Private Reader");
    expect(isHelpContext("reader")).toBe(true);
    expect(helpIndex).toContain("[[Help - Private Reader]]");
    expect(readerHelp).toContain("not copied into your graph");
    expect(readerHelp).toContain("[[Book title]]");
    expect(readerHelp).toContain("unverified");
    expect(readerHelp).toContain("screen-content");
    expect(studiesHelp).toContain("[[Help - Private Reader]]");
    expect(booksHelp).toContain("[[Help - Private Reader]]");
    await loadHelpPage("reader");
    expect(api.invoke).toHaveBeenCalledWith("help_get_page", { context: "reader" });
  });
  it("routes Studies navigation and F1 to current resume and timing guidance", async () => {
    expect(helpPageTitle("studies")).toBe("Help - Studies");
    expect(isHelpContext("studies")).toBe(true);
    expect(appSource).toContain('studies: "studies"');
    expect(appSource).toContain('target === "__studies__"');
    expect(appSource).toContain('saved.kind === "studies"');
    expect(sidebarSource).toContain('onNavigate("__studies__")');
    expect(helpIndex).toContain("[[Help - Studies]]");
    expect(studiesHelp).toContain("90 seconds");
    expect(studiesHelp).toContain("manual checkpoint");
    expect(studiesHelp).toContain("embedding restrictions");
    expect(studiesHelp).toContain("**Search or paste a link**");
    expect(studiesHelp).toContain("Selecting a result does not save anything");
    expect(studiesHelp).toContain("**Edit details**");
    expect(studiesHelp).toContain("current topic filter");
    expect(studiesHelp).toContain("never replace a title");
    expect(studiesHelp).toContain("**+ Add a new topic...**");
    expect(studiesHelp).toContain("Pasting a link contacts YouTube");
    expect(studiesHelp).toContain("private/local network addresses");
    await loadHelpPage("studies");
    expect(api.invoke).toHaveBeenCalledWith("help_get_page", { context: "studies" });
  });
  it("routes reading-note controls to current selection and deletion guidance", async () => {
    expect(readingNotesPanel).toContain('data-help-context="editor"');
    expect(appSource).toContain('closest("[data-help-context]")');
    expect(editorHelp).toContain("blank composer automatically attaches");
    expect(editorHelp).toContain("**Delete note…**");
    expect(editorHelp).toContain("**Delete all notes on this page…**");
    expect(editorHelp).toContain("**More actions**");
    expect(editorHelp).toContain("centered confirmation dialog");
    expect(editorHelp).toContain("**Cancel** is focused first");
    expect(editorHelp).toContain("even when **Notes scope** is **All notes**");
    await loadHelpPage("editor");
    expect(api.invoke).toHaveBeenCalledWith("help_get_page", { context: "editor" });
  });
  it("explains that explicit reindex rebuilds copied books without wiping user state", () => {
    expect(booksHelp).toContain("extracts book text again");
    expect(booksHelp).toContain("external file you originally imported is no longer required");
    expect(booksHelp).toContain("flashcard review progress");
    expect(settingsHelp).toContain("all imported originals");
    expect(settingsHelp).toContain("Removed sources are cleaned from the index, not recreated.");
  });
  it("explains explicit revision-checked sync choices, including deletion and binary files", () => {
    expect(syncResolution).toContain('data-help-context="sync"');
    expect(syncHelp).toContain("**Confirm choice**");
    expect(syncHelp).toContain("**deleted**");
    expect(syncHelp).toContain("sync-recovery/");
    expect(syncHelp).toContain("strong server ETag");
    expect(syncHelp).not.toContain("into the version you want, and save.");
  });
  it("routes original books and the import chooser to source-preservation guidance", async () => {
    expect(isHelpContext("books")).toBe(true);
    expect(helpPageTitle("books")).toBe("Help - Books");
    expect(appSource).toContain('isOriginalBookPage(currentPage) || isBookAnnotationPage(currentPage)');
    expect(appSource).toContain('name="book annotation editor"');
    expect(importDialog).toContain('data-help-context="books"');
    expect(helpIndex).toContain("[[Help - Books]]");
    expect(booksHelp).toContain("Unchecked (default)");
    expect(booksHelp).toContain("Convert to editable Markdown");
    expect(booksHelp).toContain("`1.jsonld`");
    expect(booksHelp).toContain("Resolve with merged note");
    expect(booksHelp).toContain("deletion versus an");
    expect(booksHelp).toContain("source stays unchanged");
    expect(booksHelp).toContain("rebuildable index");
    expect(booksHelp).toContain("Reimporting a valid pair combines revision");
    expect(booksHelp).toContain("Malformed or mismatched companions reject the import");
    await loadHelpPage("books");
    expect(api.invoke).toHaveBeenCalledWith("help_get_page", { context: "books" });
  });
  it("routes the single graph-search dialog to its current F1 guidance", () => {
    expect(searchSource).toContain('data-help-context="search"');
    expect(helpPageTitle("search")).toBe("Help - Search");
    expect(searchHelp).toContain("**Search your graph**");
    expect(searchHelp).toContain("same dialog");
    expect(searchHelp).not.toContain("Use sidebar search");
  });

  it("routes F1 in recovery controls to current bundled AI guidance", async () => {
    expect(helpPageTitle("ai")).toBe("AI Setup And Privacy");
    expect(isHelpContext("ai")).toBe(true);
    expect(isHelpContext("constructor")).toBe(false);
    expect(recoverySource).toContain('data-help-context="ai"');
    expect(appSource).toContain('closest("[data-help-context]")');
    await loadHelpPage("ai");
    expect(api.invoke).toHaveBeenCalledWith("help_get_page", { context: "ai" });
  });
  it("links Chat's runtime warnings to GPU fallback and shutdown guidance", () => {
    expect(helpPageTitle("chat")).toBe("Help - Chat");
    expect(chatHelp).toContain("[[AI Setup And Privacy]]");
    expect(chatHelp).toContain("GPU");
    expect(aiHelp).toContain("queued and active");
    expect(aiHelp).toContain("not GPU driver or");
    expect(aiHelp).toContain("uses CPU");
  });
  it("maps every supported context to a seeded help page", () => {
    const contexts: HelpContext[] = [
      "general",
      "editor",
      "journal",
      "graph",
      "flashcards",
      "tasks",
      "studies",
      "chat",
      "settings",
      "ai",
      "sync",
      "search",
      "books",
      "reader",
    ];

    for (const context of contexts) {
      expect(helpPageTitle(context)).toMatch(/^Grafium Help$|^Help - |^AI Setup And Privacy$/);
    }
  });
});
