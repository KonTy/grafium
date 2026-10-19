import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import SpellcheckMenu from "./SpellcheckMenu.svelte";
import { editorHasDialogFocus } from "../lib/editorDialogFocus";
import { spellcheck } from "../lib/spellcheck";
import { toasts } from "../lib/toast.svelte";

vi.mock("../lib/spellcheck", () => ({
  spellcheck: { suggest: vi.fn(), addWord: vi.fn(), ignore: vi.fn() },
}));

const DOC = "a recieve b";

let host: HTMLDivElement;
let view: EditorView;
let component: ReturnType<typeof mount> | null = null;
let userEvents: string[];
const onClose = vi.fn();

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
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
  vi.mocked(spellcheck.suggest).mockResolvedValue(["receive", "relieve"]);
});

afterEach(async () => {
  if (component) await unmount(component);
  component = null;
  host.remove();
  view.destroy();
  toasts.splice(0);
  vi.resetAllMocks();
});

const settle = async () => {
  await Promise.resolve();
  await tick();
  flushSync();
};

async function open() {
  component = mount(SpellcheckMenu, {
    target: host,
    props: {
      request: { view, from: 2, to: 9, word: "recieve", clientX: 30, clientY: 40 },
      onClose,
    },
  });
  flushSync();
  await settle();
}

const menu = () => host.querySelector<HTMLElement>('[role="menu"]')!;
const labels = () =>
  [...host.querySelectorAll('[role="menuitem"]')].map((item) => item.textContent?.trim());
function item(label: string): HTMLElement {
  const found = [...host.querySelectorAll<HTMLElement>('[role="menuitem"]')].find(
    (candidate) => candidate.textContent?.trim() === label,
  );
  expect(found, `menu item: ${label}`).toBeDefined();
  return found!;
}

describe("the suggestions", () => {
  it("are listed best first once they arrive", async () => {
    let deliver!: (words: string[]) => void;
    vi.mocked(spellcheck.suggest).mockReturnValue(
      new Promise<string[]>((resolve) => {
        deliver = resolve;
      }),
    );
    await open();
    expect(spellcheck.suggest).toHaveBeenCalledWith("recieve");
    expect(labels()).toEqual(["Looking for suggestions…", "Add to dictionary", "Ignore for now"]);

    deliver(["receive", "relieve"]);
    await settle();
    expect(labels()).toEqual(["receive", "relieve", "Add to dictionary", "Ignore for now"]);
  });

  it("say so when there are none, and the other choices stay", async () => {
    vi.mocked(spellcheck.suggest).mockResolvedValue([]);
    await open();
    expect(labels()).toEqual(["No suggestions", "Add to dictionary", "Ignore for now"]);
  });

  it("replace the word, leave the caret after it and give the focus back", async () => {
    await open();
    item("relieve").click();
    expect(view.state.doc.toString()).toBe("a relieve b");
    expect(view.state.selection.main.head).toBe(9);
    expect(userEvents).toEqual(["input.spellcheck"]);
    expect(onClose).toHaveBeenCalledOnce();
    expect(document.activeElement).toBe(view.contentDOM);
  });

  it("do nothing, and say so, if the text changed while the menu was open", async () => {
    await open();
    view.dispatch({ changes: { from: 0, insert: "zz" } });
    item("receive").click();
    expect(view.state.doc.toString()).toBe("zza recieve b");
    expect(toasts.map((toast) => toast.message)).toEqual([
      "The text changed, so the correction was not applied.",
    ]);
    expect(onClose).toHaveBeenCalledOnce();
  });
});

describe("the other choices", () => {
  it("add the word to the dictionary", async () => {
    vi.mocked(spellcheck.addWord).mockResolvedValue(["recieve"]);
    await open();
    item("Add to dictionary").click();
    await settle();
    expect(spellcheck.addWord).toHaveBeenCalledWith("recieve");
    expect(onClose).toHaveBeenCalledOnce();
    expect(toasts).toHaveLength(0);
  });

  it("say why a word could not be added", async () => {
    vi.mocked(spellcheck.addWord).mockRejectedValue("disk full");
    await open();
    item("Add to dictionary").click();
    await settle();
    expect(toasts.map(({ message, severity }) => [message, severity])).toEqual([
      ['Could not add "recieve" to the dictionary: disk full', "error"],
    ]);
  });

  it("ignore the word for now", async () => {
    await open();
    item("Ignore for now").click();
    expect(spellcheck.ignore).toHaveBeenCalledWith("recieve");
    expect(onClose).toHaveBeenCalledOnce();
    expect(document.activeElement).toBe(view.contentDOM);
  });
});

describe("the menu", () => {
  it("takes the focus, moves through its items with the arrows and closes on Escape", async () => {
    await open();
    expect(document.activeElement).toBe(menu());

    const press = (key: string) =>
      menu().dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
    press("ArrowDown");
    expect(document.activeElement).toBe(item("receive"));
    press("End");
    expect(document.activeElement).toBe(item("Ignore for now"));
    expect(onClose).not.toHaveBeenCalled();

    press("Escape");
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("closes on a click or right-click anywhere else, but not on one inside itself", async () => {
    await open();
    const inside = new Event("pointerdown", { bubbles: true, cancelable: true });
    item("receive").dispatchEvent(inside);
    expect(inside.defaultPrevented).toBe(true);
    expect(onClose).not.toHaveBeenCalled();

    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(onClose).toHaveBeenCalledTimes(1);
    document.body.dispatchEvent(new Event("contextmenu", { bubbles: true }));
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("holds the editor open before it takes the focus, and lets go when it closes", async () => {
    view.contentDOM.focus();
    expect(document.activeElement).toBe(view.contentDOM);
    const retainedWhenBlurred: boolean[] = [];
    view.contentDOM.addEventListener("blur", () => {
      retainedWhenBlurred.push(editorHasDialogFocus(view.dom));
    });

    await open();
    expect(retainedWhenBlurred).toEqual([true]);
    expect(editorHasDialogFocus(view.dom)).toBe(true);

    await unmount(component!);
    component = null;
    expect(editorHasDialogFocus(view.dom)).toBe(false);
    expect(document.activeElement).toBe(view.contentDOM);
  });

  it("stays inside the window", async () => {
    vi.stubGlobal("innerWidth", 400);
    vi.stubGlobal("innerHeight", 300);
    try {
      component = mount(SpellcheckMenu, {
        target: host,
        props: {
          request: { view, from: 2, to: 9, word: "recieve", clientX: 5000, clientY: 5000 },
          onClose,
        },
      });
      flushSync();
      await settle();
      const left = parseFloat(menu().style.left);
      const top = parseFloat(menu().style.top);
      expect(left).toBeLessThan(400);
      expect(top).toBeLessThan(300);
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
