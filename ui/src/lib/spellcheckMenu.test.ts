import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import ContextMenuHost from "../components/ContextMenuHost.svelte";
import { closeContextMenu, getOpenContextMenu, openContextMenu } from "./contextMenuHost.svelte";
import type { ContextMenuAction, ContextMenuSection } from "./contextMenuSections";
import { editorHasDialogFocus } from "./editorDialogFocus";
import { spellcheck } from "./spellcheck";
import { buildSpellSections, type SpellMenuSource } from "./spellcheckMenu";
import { toasts } from "./toast.svelte";

vi.mock("./editActions", () => ({ buildEditSection: () => null }));
vi.mock("./spellcheck", () => ({
  spellcheck: { suggest: vi.fn(), addWord: vi.fn(), ignore: vi.fn() },
}));

const DOC = "a recieve b";
const WORD = { from: 2, to: 9, word: "recieve" };

let view: EditorView;
let userEvents: string[];
let source: SpellMenuSource & {
  suggest: ReturnType<typeof vi.fn>;
  addWord: ReturnType<typeof vi.fn>;
  ignore: ReturnType<typeof vi.fn>;
};

beforeEach(() => {
  userEvents = [];
  view = new EditorView({
    parent: document.body,
    state: EditorState.create({
      doc: DOC,
      extensions: [
        EditorView.updateListener.of((update) => {
          for (const transaction of update.transactions) {
            if (transaction.isUserEvent("input.spellcheck")) userEvents.push("input.spellcheck");
          }
        }),
      ],
    }),
  });
  // jsdom only focuses elements it considers focusable.
  view.contentDOM.tabIndex = 0;
  source = {
    suggest: vi.fn().mockResolvedValue(["receive", "relieve"]),
    addWord: vi.fn().mockResolvedValue(["recieve"]),
    ignore: vi.fn(),
  };
});

afterEach(() => {
  view.destroy();
  toasts.splice(0);
  document.body.innerHTML = "";
  vi.resetAllMocks();
});

function sections(): ContextMenuSection[] {
  return buildSpellSections({ view, ...WORD }, source);
}

function choose(section: ContextMenuSection, label: string): ContextMenuAction {
  const found = section.items.find((item) => item.label === label);
  expect(found, `menu item: ${label}`).toBeDefined();
  expect(found!.kind ?? "action").toBe("action");
  return found as ContextMenuAction;
}

async function suggestionsOf(): Promise<ContextMenuSection> {
  const [spelling] = sections();
  return { ...spelling, items: await spelling.resolve!() };
}

describe("the spelling sections", () => {
  it("put the suggestions first and the dictionary choices after them", () => {
    const [spelling, actions] = sections();
    expect(spelling).toMatchObject({ id: "spelling", ariaLabel: "Spelling suggestions" });
    expect(actions).toMatchObject({ id: "spelling-actions", ariaLabel: "Dictionary" });
    expect(spelling.order).toBeLessThan(actions.order!);
    expect(actions.order).toBeLessThan(50);
    expect(actions.items.map((item) => item.label)).toEqual(["Add to dictionary", "Ignore for now"]);
  });

  it("show a waiting line until the suggestions arrive", () => {
    const [spelling] = sections();
    expect(spelling.items).toEqual([
      { kind: "status", id: "spelling.loading", label: "Looking for suggestions…" },
    ]);
    expect(source.suggest).not.toHaveBeenCalled();
  });

  it("list the suggestions best first once asked", async () => {
    const spelling = await suggestionsOf();
    expect(source.suggest).toHaveBeenCalledWith("recieve");
    expect(spelling.items.map((item) => item.label)).toEqual(["receive", "relieve"]);
    expect(spelling.items.every((item) => (item as ContextMenuAction).emphasis)).toBe(true);
  });

  it("say so when there are none", async () => {
    source.suggest.mockResolvedValue([]);
    const spelling = await suggestionsOf();
    expect(spelling.items).toEqual([{ kind: "status", id: "spelling.none", label: "No suggestions" }]);
  });

  it("say so when the suggestions could not be fetched", () => {
    const [spelling] = sections();
    expect(spelling.errorLabel).toBe("Suggestions unavailable");
  });

  it("ask the shared spell checker when no other source is given", async () => {
    vi.mocked(spellcheck.suggest).mockResolvedValue(["receive"]);
    const [spelling] = buildSpellSections({ view, ...WORD });
    const items = await spelling.resolve!();
    expect(spellcheck.suggest).toHaveBeenCalledWith("recieve");
    expect(items.map((item) => item.label)).toEqual(["receive"]);
  });
});

describe("choosing a suggestion", () => {
  it("replaces the word, leaves the caret after it and gives the focus back", async () => {
    const spelling = await suggestionsOf();
    void choose(spelling, "relieve").onSelect();
    expect(view.state.doc.toString()).toBe("a relieve b");
    expect(view.state.selection.main.head).toBe(9);
    expect(userEvents).toEqual(["input.spellcheck"]);
    expect(document.activeElement).toBe(view.contentDOM);
  });

  it("does nothing, and says so, if the text changed while the menu was open", async () => {
    const spelling = await suggestionsOf();
    view.dispatch({ changes: { from: 0, insert: "zz" } });
    void choose(spelling, "receive").onSelect();
    expect(view.state.doc.toString()).toBe("zza recieve b");
    expect(userEvents).toEqual([]);
    expect(toasts.map(({ message, severity }) => [message, severity])).toEqual([
      ["The text changed, so the correction was not applied.", "info"],
    ]);
  });
});

describe("the dictionary choices", () => {
  it("add the word to the dictionary", async () => {
    const [, actions] = sections();
    await choose(actions, "Add to dictionary").onSelect();
    expect(source.addWord).toHaveBeenCalledWith("recieve");
    expect(toasts).toHaveLength(0);
    expect(document.activeElement).toBe(view.contentDOM);
  });

  it("say why a word could not be added", async () => {
    source.addWord.mockRejectedValue("disk full");
    const [, actions] = sections();
    await choose(actions, "Add to dictionary").onSelect();
    expect(toasts.map(({ message, severity }) => [message, severity])).toEqual([
      ['Could not add "recieve" to the dictionary: disk full', "error"],
    ]);
  });

  it("ignore the word for now and give the focus back", async () => {
    const [, actions] = sections();
    await choose(actions, "Ignore for now").onSelect();
    expect(source.ignore).toHaveBeenCalledWith("recieve");
    expect(document.activeElement).toBe(view.contentDOM);
  });
});

describe("in the menu", () => {
  let menu: ReturnType<typeof mount> | null = null;

  const settle = async () => {
    await Promise.resolve();
    await tick();
    flushSync();
  };

  const items = () => [...document.querySelectorAll<HTMLElement>(".ctx-menu [role='menuitem']")];
  const labels = () => items().map((el) => el.querySelector(".ctx-menu-label")?.textContent?.trim() ?? el.textContent?.trim());

  async function open() {
    menu = mount(ContextMenuHost, { target: document.body });
    flushSync();
    view.contentDOM.focus();
    openContextMenu({
      anchor: { x: 30, y: 40 },
      sections: sections(),
      target: view.contentDOM,
      returnFocus: view.contentDOM,
    });
    await settle();
  }

  afterEach(() => {
    closeContextMenu({ restoreFocus: false });
    flushSync();
    if (menu) unmount(menu);
    menu = null;
  });

  it("shows the waiting line, then the suggestions above the dictionary choices", async () => {
    let deliver!: (words: string[]) => void;
    source.suggest.mockReturnValue(
      new Promise<string[]>((resolve) => {
        deliver = resolve;
      }),
    );
    await open();
    expect(labels()).toEqual(["Looking for suggestions…", "Add to dictionary", "Ignore for now"]);

    deliver(["receive", "relieve"]);
    await settle();
    expect(labels()).toEqual(["receive", "relieve", "Add to dictionary", "Ignore for now"]);
  });

  it("says the suggestions are unavailable when the lookup fails, and keeps the dictionary choices", async () => {
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    source.suggest.mockRejectedValue(new Error("dictionary not loaded"));
    await open();
    expect(labels()).toEqual(["Suggestions unavailable", "Add to dictionary", "Ignore for now"]);
    expect(log).toHaveBeenCalledTimes(1);
    log.mockRestore();
  });

  it("replaces the word when a suggestion is clicked, and closes with the focus back in the editor", async () => {
    await open();
    items().find((el) => el.textContent?.includes("relieve"))!.click();
    await settle();
    expect(view.state.doc.toString()).toBe("a relieve b");
    expect(getOpenContextMenu()).toBeNull();
    expect(document.querySelector(".ctx-menu")).toBeNull();
    expect(document.activeElement).toBe(view.contentDOM);
    expect(editorHasDialogFocus(view.dom)).toBe(false);
  });

  it("keeps the editor from treating the menu as a blur while it is open", async () => {
    view.contentDOM.focus();
    const retainedWhenBlurred: boolean[] = [];
    view.contentDOM.addEventListener("blur", () => {
      retainedWhenBlurred.push(editorHasDialogFocus(view.dom));
    });
    await open();
    expect(retainedWhenBlurred).toEqual([true]);
    expect(editorHasDialogFocus(view.dom)).toBe(true);
  });
});
