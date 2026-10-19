import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { markdown } from "@codemirror/lang-markdown";
import { ensureSyntaxTree } from "@codemirror/language";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import {
  SETTLE_DELAY_MS,
  TYPING_DELAY_MS,
  spellcheckExtension,
  syntaxSkips,
  visibleMisspellings,
  type SpellSource,
} from "./spellcheckExtension";

function word(text: string, from: number) {
  return { word: text, from, to: from + text.length };
}

describe("visibleMisspellings", () => {
  const words = [word("recieve", 0), word("teh", 8), word("Recieve", 12)];
  const bad = new Set(["recieve", "teh", "Recieve"]);

  it("keeps the words found misspelled and nothing else", () => {
    expect(visibleMisspellings(words, new Set(["teh"]), () => false, null)).toEqual([words[1]]);
  });

  it("leaves out words the user chose to ignore", () => {
    const ignored = (text: string) => text.toLowerCase() === "recieve";
    expect(visibleMisspellings(words, bad, ignored, null)).toEqual([words[1]]);
  });

  it("leaves out the word the caret is in or touches", () => {
    expect(visibleMisspellings(words, bad, () => false, 3)).toEqual([words[1], words[2]]);
    expect(visibleMisspellings(words, bad, () => false, 0)).toEqual([words[1], words[2]]);
    expect(visibleMisspellings(words, bad, () => false, 7)).toEqual([words[1], words[2]]);
  });

  it("keeps a word the caret has moved past", () => {
    expect(visibleMisspellings([word("teh", 0)], new Set(["teh"]), () => false, 4)).toHaveLength(1);
  });
});

describe("syntaxSkips", () => {
  function skipped(doc: string): string[] {
    const state = EditorState.create({ doc, extensions: [markdown()] });
    const tree = ensureSyntaxTree(state, doc.length, 1000)!;
    return syntaxSkips(tree, 0, doc.length).map(([from, to]) => doc.slice(from, to));
  }

  it("covers inline code, fenced code and html", () => {
    expect(skipped("a `code` b <i>x</i>")).toEqual(["`code`", "<i>", "</i>"]);
    expect(skipped("a\n\n```js\nlet x\n```\n\nb")).toEqual(["```js\nlet x\n```"]);
  });

  it("is relative to where it starts", () => {
    const doc = "one `two` three";
    const state = EditorState.create({ doc, extensions: [markdown()] });
    const tree = ensureSyntaxTree(state, doc.length, 1000)!;
    expect(syntaxSkips(tree, 4, doc.length)).toEqual([[0, 5]]);
  });
});

function fakeSource(bad: string[] = ["recieve"]) {
  const listeners = new Set<() => void>();
  const ignored = new Set<string>();
  const status = { active: true };
  const check = vi.fn(
    async (words: readonly string[]) => new Set(words.filter((text) => bad.includes(text))),
  );
  const source: SpellSource = {
    ready: async () => {},
    isActive: () => status.active,
    check,
    isIgnored: (text) => ignored.has(text.toLowerCase()),
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
  const notify = () => [...listeners].forEach((listener) => listener());
  return {
    source,
    check,
    listeners,
    setActive(active: boolean) {
      status.active = active;
      notify();
    },
    ignore(text: string) {
      ignored.add(text.toLowerCase());
      notify();
    },
  };
}

const views: EditorView[] = [];

function editor(doc: string, source: SpellSource, onContextMenu = vi.fn()) {
  const view = new EditorView({
    parent: document.body,
    state: EditorState.create({
      doc,
      extensions: [markdown(), spellcheckExtension({ source, onContextMenu })],
    }),
  });
  views.push(view);
  return view;
}

function underlined(view: EditorView): (string | null)[] {
  return [...view.dom.querySelectorAll(".cm-spell-error")].map((el) => el.textContent);
}

function rightClick(target: Element, init: MouseEventInit = {}) {
  const event = new MouseEvent("contextmenu", {
    bubbles: true,
    cancelable: true,
    clientX: 40,
    clientY: 50,
    ...init,
  });
  const stop = vi.spyOn(event, "stopPropagation");
  target.dispatchEvent(event);
  return { event, stop };
}

const settle = (ms = 1) => vi.advanceTimersByTimeAsync(ms);

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("spellcheckExtension", () => {
  it("underlines misspelled words after the first pass", async () => {
    const { source } = fakeSource();
    const view = editor("I recieve mail\nand recieve more", source);
    expect(underlined(view)).toEqual([]);
    await settle();
    expect(underlined(view)).toEqual(["recieve", "recieve"]);
  });

  it("leaves code, links, tags and addresses alone", async () => {
    const { source } = fakeSource();
    const view = editor(
      [
        "Plain recieve here.",
        "",
        "`recieve` and [[recieve]] and #recieve and https://recieve.example/recieve",
        "",
        "```js",
        "recieve",
        "```",
        "",
        '<img alt="recieve" src="recieve.png"> and [link](https://example.com/recieve)',
        "",
        "<span>recieve</span> is prose between tags",
      ].join("\n"),
      source,
    );
    await settle();
    const text = view.state.doc.toString();
    const at = [...view.dom.querySelectorAll(".cm-spell-error")].map((el) => view.posAtDOM(el));
    expect(at).toEqual([text.indexOf("recieve"), text.indexOf("<span>recieve") + "<span>".length]);
  });

  it("does nothing while the checker is off, and starts when it is turned on", async () => {
    const fake = fakeSource();
    fake.setActive(false);
    const view = editor("a recieve b", fake.source);
    await settle();
    expect(underlined(view)).toEqual([]);
    expect(fake.check).not.toHaveBeenCalled();

    fake.setActive(true);
    await settle();
    expect(underlined(view)).toEqual(["recieve"]);

    fake.setActive(false);
    await settle();
    expect(underlined(view)).toEqual([]);
  });

  it("stays quiet while the editor cannot be edited, and checks once it can", async () => {
    const mode = new Compartment();
    const { source } = fakeSource();
    const view = new EditorView({
      parent: document.body,
      state: EditorState.create({
        doc: "a recieve b",
        extensions: [
          markdown(),
          mode.of([EditorState.readOnly.of(true), EditorView.editable.of(false)]),
          spellcheckExtension({ source, onContextMenu: vi.fn() }),
        ],
      }),
    });
    views.push(view);
    await settle();
    expect(underlined(view)).toEqual([]);

    view.dispatch({ effects: mode.reconfigure([]) });
    await settle(SETTLE_DELAY_MS);
    expect(underlined(view)).toEqual(["recieve"]);

    view.dispatch({ effects: mode.reconfigure(EditorState.readOnly.of(true)) });
    await settle(SETTLE_DELAY_MS);
    expect(underlined(view)).toEqual([]);
  });

  it("checks again after an edit, but only once typing pauses", async () => {
    const fake = fakeSource();
    const view = editor("fine words", fake.source);
    await settle();
    fake.check.mockClear();

    view.dispatch({ changes: { from: view.state.doc.length, insert: " recieve more" } });
    await settle(TYPING_DELAY_MS - 1);
    expect(fake.check).not.toHaveBeenCalled();
    await settle(1);
    expect(underlined(view)).toEqual(["recieve"]);
  });

  it("removes the underline once the word is fixed", async () => {
    const { source } = fakeSource();
    const view = editor("a recieve b", source);
    await settle();
    expect(underlined(view)).toEqual(["recieve"]);

    view.dispatch({ changes: { from: 2, to: 9, insert: "receive" } });
    await settle(TYPING_DELAY_MS);
    expect(underlined(view)).toEqual([]);
  });

  it("keeps an underline on its word while the text before it changes", async () => {
    const { source } = fakeSource();
    const view = editor("a recieve b", source);
    await settle();

    view.dispatch({ changes: { from: 0, insert: "zzzz " } });
    expect(underlined(view)).toEqual(["recieve"]);
  });

  it("stops underlining a word the user ignores", async () => {
    const fake = fakeSource();
    const view = editor("a recieve b", fake.source);
    await settle();
    fake.ignore("Recieve");
    await settle();
    expect(underlined(view)).toEqual([]);
  });

  it("does not underline the word being typed until the caret moves on", async () => {
    const { source } = fakeSource();
    vi.spyOn(EditorView.prototype, "hasFocus", "get").mockReturnValue(true);
    const view = editor("a ", source);
    view.dispatch({ selection: { anchor: 2 } });
    await settle();

    view.dispatch({ changes: { from: 2, insert: "recieve" }, selection: { anchor: 9 } });
    await settle(TYPING_DELAY_MS);
    expect(underlined(view)).toEqual([]);

    view.dispatch({ changes: { from: 9, insert: " " }, selection: { anchor: 10 } });
    await settle(TYPING_DELAY_MS);
    expect(underlined(view)).toEqual(["recieve"]);
  });

  it("underlines the word again when the caret moves inside it", async () => {
    const { source } = fakeSource();
    vi.spyOn(EditorView.prototype, "hasFocus", "get").mockReturnValue(true);
    const view = editor("a ", source);
    await settle();

    view.dispatch({ changes: { from: 2, insert: "recieve" }, selection: { anchor: 9 } });
    await settle(TYPING_DELAY_MS);
    expect(underlined(view)).toEqual([]);

    view.dispatch({ selection: { anchor: 4 } });
    await settle(SETTLE_DELAY_MS);
    expect(underlined(view)).toEqual(["recieve"]);
  });

  it("waits for an input method to finish composing", async () => {
    const fake = fakeSource();
    const view = editor("a recieve b", fake.source);
    view.contentDOM.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    expect(view.compositionStarted).toBe(true);
    await settle(TYPING_DELAY_MS * 2);
    expect(fake.check).not.toHaveBeenCalled();

    view.contentDOM.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true }));
    await settle(TYPING_DELAY_MS * 2);
    expect(underlined(view)).toEqual(["recieve"]);
  });

  it("drops an answer that arrives after the text has moved on", async () => {
    let answer!: (bad: Set<string>) => void;
    const late = new Promise<Set<string>>((resolve) => {
      answer = resolve;
    });
    const fake = fakeSource();
    fake.check.mockReturnValueOnce(late);
    const view = editor("a recieve b", fake.source);
    await settle();

    view.dispatch({ changes: { from: 0, insert: "zzzz " } });
    answer(new Set(["recieve"]));
    await settle();
    expect(underlined(view)).toEqual([]);

    await settle(TYPING_DELAY_MS);
    expect(underlined(view)).toEqual(["recieve"]);
  });

  it("checks only the first lines of an enormous viewport", async () => {
    const fake = fakeSource();
    const long = "alpha ".repeat(10_000);
    editor(`${long}\n${long}\ntail recieve`, fake.source);
    await settle();
    expect(fake.check).toHaveBeenCalledTimes(1);
    const words = fake.check.mock.calls[0][0];
    expect(words).toContain("alpha");
    expect(words).not.toContain("tail");
  });

  it("keeps working when the checker throws", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const fake = fakeSource();
    fake.check.mockRejectedValueOnce(new Error("backend is gone"));
    const view = editor("a recieve b", fake.source);
    await settle();
    expect(warn).toHaveBeenCalled();
    expect(underlined(view)).toEqual([]);

    view.dispatch({ changes: { from: 0, insert: "more " } });
    await settle(TYPING_DELAY_MS);
    expect(underlined(view)).toEqual(["recieve"]);
  });

  it("stops listening and checking once the editor is destroyed", async () => {
    const fake = fakeSource();
    const view = editor("a recieve b", fake.source);
    expect(fake.listeners.size).toBe(1);
    view.destroy();
    expect(fake.listeners.size).toBe(0);
    await settle(1000);
    expect(fake.check).not.toHaveBeenCalled();
  });
});

describe("right-clicking an underlined word", () => {
  async function ready(doc = "a recieve b") {
    const onContextMenu = vi.fn();
    const view = editor(doc, fakeSource().source, onContextMenu);
    await settle();
    return { view, onContextMenu, span: view.dom.querySelector(".cm-spell-error")! };
  }

  it("reports the word and where the click was, and keeps the browser menu away", async () => {
    const { view, onContextMenu, span } = await ready();
    const { event, stop } = rightClick(span);
    expect(onContextMenu).toHaveBeenCalledOnce();
    expect(onContextMenu).toHaveBeenCalledWith({
      view,
      from: 2,
      to: 9,
      word: "recieve",
      clientX: 40,
      clientY: 50,
    });
    expect(event.defaultPrevented).toBe(true);
    expect(stop).toHaveBeenCalled();
  });

  it("is left alone anywhere else", async () => {
    const { view, onContextMenu } = await ready();
    const { event } = rightClick(view.dom.querySelector(".cm-line")!);
    expect(onContextMenu).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });

  it("is left to the editor when something else is selected", async () => {
    const { view, onContextMenu, span } = await ready();
    view.dispatch({ selection: { anchor: 0, head: 1 } });
    const { event } = rightClick(span);
    expect(onContextMenu).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });

  it("works when the word itself is selected", async () => {
    const { view, onContextMenu, span } = await ready();
    view.dispatch({ selection: { anchor: 2, head: 9 } });
    rightClick(span);
    expect(onContextMenu).toHaveBeenCalledOnce();
  });
});
