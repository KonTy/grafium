import { afterEach, describe, expect, it, vi } from "vitest";
import { keymap_manager, registerDefaultShortcuts } from "./keymap";

function keyEvent(init: KeyboardEventInit): KeyboardEvent {
  return new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
}

function stubActions() {
  return {
    goJournal: vi.fn(),
    goJournalEdit: vi.fn(),
    goHome: vi.fn(),
    goAllPages: vi.fn(),
    goGraph: vi.fn(),
    goFlashcards: vi.fn(),
    goTomorrow: vi.fn(),
    goTasks: vi.fn(),
    goChat: vi.fn(),
    goNextJournal: vi.fn(),
    goPrevJournal: vi.fn(),
    goForward: vi.fn(),
    goBackward: vi.fn(),
    search: vi.fn(),
    searchInPage: vi.fn(),
    focusLocalSearch: vi.fn(),
    toggleSidebar: vi.fn(),
    toggleRightSidebar: vi.fn(),
    toggleTheme: vi.fn(),
    toggleHelp: vi.fn(),
    toggleSettings: vi.fn(),
    toggleWideMode: vi.fn(),
    toggleZenMode: vi.fn(),
    newPage: vi.fn(),
    reindex: vi.fn(),
    undo: vi.fn(),
    redo: vi.fn(),
    commandPalette: vi.fn(),
    importMedia: vi.fn(),
    importBooks: vi.fn(),
    insertTimeStamp: vi.fn(),
    insertPersonalJournal: vi.fn(),
  };
}

afterEach(() => {
  keymap_manager.register([]);
  keymap_manager.isEditing = false;
});

describe("keymap dual-mode matching", () => {
  it("keeps vim chords nav-only and runs modifier aliases while editing", () => {
    const actions = stubActions();
    registerDefaultShortcuts(actions);
    keymap_manager.isEditing = true;

    expect(keymap_manager.handleKeydown(keyEvent({ key: "g" }))).toBe(false);
    expect(keymap_manager.handleKeydown(keyEvent({ key: "j" }))).toBe(false);
    expect(actions.goJournal).not.toHaveBeenCalled();

    const combo = keyEvent({ key: "j", ctrlKey: true, shiftKey: true });
    expect(keymap_manager.handleKeydown(combo)).toBe(true);
    expect(actions.goJournalEdit).toHaveBeenCalledTimes(1);
    expect(actions.goJournal).not.toHaveBeenCalled();
  });

  it("does not treat Ctrl-K as Ctrl-Shift-K", () => {
    const actions = stubActions();
    registerDefaultShortcuts(actions);

    expect(keymap_manager.handleKeydown(keyEvent({ key: "k", ctrlKey: true }))).toBe(true);
    expect(actions.search).toHaveBeenCalledTimes(1);
    expect(actions.searchInPage).not.toHaveBeenCalled();

    expect(keymap_manager.handleKeydown(keyEvent({ key: "k", ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(actions.searchInPage).toHaveBeenCalledTimes(1);
    expect(actions.search).toHaveBeenCalledTimes(1);
  });

  it("matches Ctrl-> / Ctrl-< via Period and Comma codes", () => {
    const actions = stubActions();
    registerDefaultShortcuts(actions);

    const next = keyEvent({ key: ">", code: "Period", ctrlKey: true, shiftKey: true });
    expect(keymap_manager.handleKeydown(next)).toBe(true);
    expect(actions.goNextJournal).toHaveBeenCalledTimes(1);

    const prev = keyEvent({ key: "<", code: "Comma", ctrlKey: true, shiftKey: true });
    expect(keymap_manager.handleKeydown(prev)).toBe(true);
    expect(actions.goPrevJournal).toHaveBeenCalledTimes(1);
  });

  it("matches Ctrl-Shift-C via KeyC", () => {
    const actions = stubActions();
    registerDefaultShortcuts(actions);
    const event = keyEvent({ key: "C", code: "KeyC", ctrlKey: true, shiftKey: true });
    expect(keymap_manager.handleKeydown(event)).toBe(true);
    expect(actions.goChat).toHaveBeenCalledTimes(1);
  });

  it("matches Ctrl-Alt-B regardless of modifier order", () => {
    const actions = stubActions();
    registerDefaultShortcuts(actions);
    const event = keyEvent({ key: "b", ctrlKey: true, altKey: true });
    expect(keymap_manager.handleKeydown(event)).toBe(true);
    expect(actions.toggleRightSidebar).toHaveBeenCalledTimes(1);
  });

  it("matches Alt-T and Alt-J while editing", () => {
    const actions = stubActions();
    registerDefaultShortcuts(actions);
    keymap_manager.isEditing = true;

    expect(keymap_manager.handleKeydown(keyEvent({ key: "t", code: "KeyT", altKey: true }))).toBe(true);
    expect(actions.insertTimeStamp).toHaveBeenCalledTimes(1);

    expect(keymap_manager.handleKeydown(keyEvent({ key: "j", code: "KeyJ", altKey: true }))).toBe(true);
    expect(actions.insertPersonalJournal).toHaveBeenCalledTimes(1);
  });
});
