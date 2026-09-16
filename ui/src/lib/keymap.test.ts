import { afterEach, describe, expect, it, vi } from "vitest";
import { keymap_manager, registerDefaultShortcuts } from "./keymap";

function keyEvent(init: KeyboardEventInit): KeyboardEvent {
  return new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
}

function stubActions() {
  return {
    goJournal: vi.fn(),
    goJournalDate: vi.fn(),
    goLink: vi.fn(),
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
    insertPersonalDiary: vi.fn(),
  };
}

afterEach(() => {
  keymap_manager.register([]);
  keymap_manager.isEditing = false;
});

describe("keymap dual-mode matching", () => {
  it("opens Go to link while editing and leaves modified variants alone", () => {
    const actions = stubActions();
    registerDefaultShortcuts(actions);
    keymap_manager.isEditing = true;
    const event = keyEvent({ key: "l", code: "KeyL", ctrlKey: true });
    expect(keymap_manager.handleKeydown(event)).toBe(true);
    expect(event.defaultPrevented).toBe(true);
    expect(actions.goLink).toHaveBeenCalledTimes(1);
    expect(keymap_manager.handleKeydown(keyEvent({ key: "L", code: "KeyL", ctrlKey: true, shiftKey: true }))).toBe(false);
    expect(actions.goLink).toHaveBeenCalledTimes(1);
  });

  it("opens the date calendar while editing without replacing the graph shortcut", () => {
    const actions = stubActions();
    registerDefaultShortcuts(actions);
    keymap_manager.isEditing = true;
    expect(keymap_manager.handleKeydown(keyEvent({ key: "g", ctrlKey: true }))).toBe(true);
    expect(actions.goJournalDate).toHaveBeenCalledTimes(1);
    expect(actions.goGraph).not.toHaveBeenCalled();
    expect(keymap_manager.handleKeydown(keyEvent({ key: "G", code: "KeyG", ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(actions.goGraph).toHaveBeenCalledTimes(1);
  });

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

  it("opens Chat with Alt-C while editing and ignores the old Ctrl combos", () => {
    const actions = stubActions();
    registerDefaultShortcuts(actions);
    keymap_manager.isEditing = true;
    const event = keyEvent({ key: "c", code: "KeyC", altKey: true });
    expect(keymap_manager.handleKeydown(event)).toBe(true);
    expect(event.defaultPrevented).toBe(true);
    expect(actions.goChat).toHaveBeenCalledTimes(1);
    expect(keymap_manager.handleKeydown(keyEvent({ key: "C", code: "KeyC", ctrlKey: true, shiftKey: true }))).toBe(false);
    expect(keymap_manager.handleKeydown(keyEvent({ key: "C", code: "KeyC", ctrlKey: true, altKey: true }))).toBe(false);
    expect(actions.goChat).toHaveBeenCalledTimes(1);
  });

  it.each([false, true])("toggles the right pane with Ctrl-Shift-B (editing: %s) and leaves bold to the editor", (editing) => {
    const actions = stubActions();
    registerDefaultShortcuts(actions);
    keymap_manager.isEditing = editing;
    const event = keyEvent({ key: "B", code: "KeyB", ctrlKey: true, shiftKey: true });
    expect(keymap_manager.handleKeydown(event)).toBe(true);
    expect(event.defaultPrevented).toBe(true);
    expect(actions.toggleRightSidebar).toHaveBeenCalledTimes(1);
    expect(actions.toggleSidebar).not.toHaveBeenCalled();
    expect(keymap_manager.handleKeydown(keyEvent({ key: "b", code: "KeyB", ctrlKey: true, altKey: true }))).toBe(false);
    expect(actions.toggleRightSidebar).toHaveBeenCalledTimes(1);
    expect(keymap_manager.handleKeydown(keyEvent({ key: ".", code: "Period", ctrlKey: true }))).toBe(true);
    expect(actions.toggleRightSidebar).toHaveBeenCalledTimes(2);
  });

  it("matches Alt-T and Alt-D while editing", () => {
    const actions = stubActions();
    registerDefaultShortcuts(actions);
    keymap_manager.isEditing = true;

    expect(keymap_manager.handleKeydown(keyEvent({ key: "t", code: "KeyT", altKey: true }))).toBe(true);
    expect(actions.insertTimeStamp).toHaveBeenCalledTimes(1);

    expect(keymap_manager.handleKeydown(keyEvent({ key: "d", code: "KeyD", altKey: true }))).toBe(true);
    expect(actions.insertPersonalDiary).toHaveBeenCalledTimes(1);
  });
});
