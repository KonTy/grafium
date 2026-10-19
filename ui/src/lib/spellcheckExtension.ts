import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import { Prec, type EditorState, type Extension } from "@codemirror/state";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  type DecorationSet,
  type ViewUpdate,
} from "@codemirror/view";
import { spellcheck } from "./spellcheck";
import { findSpellWords, type SpellWord, type TextRange } from "./spellcheckTokens";

/** Pause after typing before checking, so a word is not judged half-typed. */
export const TYPING_DELAY_MS = 250;
/** Pause after scrolling or a parser update, which do not change the text. */
export const SETTLE_DELAY_MS = 120;
/** The most text checked in one pass. */
const MAX_REGION_CHARS = 100_000;
/** The longest one pass waits for the parser. */
const PARSE_BUDGET_MS = 30;

const MISSPELLED_CLASS = "cm-spell-error";
const MISSPELLED = Decoration.mark({ class: MISSPELLED_CLASS });

/** Markdown syntax that is code or markup rather than prose. */
const NON_PROSE_NODES = new Set([
  "FencedCode",
  "CodeBlock",
  "InlineCode",
  "URL",
  "Autolink",
  "HTMLTag",
  "HTMLBlock",
  "Comment",
  "CommentBlock",
  "ProcessingInstruction",
  "ProcessingInstructionBlock",
  "LinkReference",
  "LinkLabel",
  "Entity",
]);

type SyntaxTree = NonNullable<ReturnType<typeof ensureSyntaxTree>>;

/** What the editor needs from the spell checker; `SpellcheckService` provides it. */
export interface SpellSource {
  ready(): Promise<void>;
  isActive(): boolean;
  check(words: readonly string[]): Promise<Set<string>>;
  isIgnored(word: string): boolean;
  subscribe(listener: () => void): () => void;
}

/** A right-click on an underlined word. */
export interface SpellcheckRequest {
  view: EditorView;
  /** Where the word starts and ends in the document. */
  from: number;
  to: number;
  word: string;
  clientX: number;
  clientY: number;
}

export interface SpellcheckOptions {
  onContextMenu(request: SpellcheckRequest): void;
  /** Defaults to the app's spell checker. */
  source?: SpellSource;
}

/** The parts of `from..to` that the syntax tree says are not prose, relative to `from`. */
export function syntaxSkips(tree: SyntaxTree, from: number, to: number): TextRange[] {
  const ranges: TextRange[] = [];
  tree.iterate({
    from,
    to,
    enter(node) {
      if (!NON_PROSE_NODES.has(node.name)) return;
      ranges.push([node.from - from, node.to - from]);
      return false;
    },
  });
  return ranges;
}

/**
 * The words to underline: those found misspelled, except ones the user chose to
 * ignore and the word being typed at `caret`, which is not finished yet.
 */
export function visibleMisspellings(
  words: readonly SpellWord[],
  misspelled: ReadonlySet<string>,
  isIgnored: (word: string) => boolean,
  caret: number | null,
): SpellWord[] {
  return words.filter(
    (word) =>
      misspelled.has(word.word) &&
      !isIgnored(word.word) &&
      !(caret !== null && word.from <= caret && caret <= word.to),
  );
}

/**
 * Squiggles are for fixing text, so they wait until it can be edited. That
 * leaves out a page editor between blocks and any editor locked for a write.
 */
function isWritable(state: EditorState): boolean {
  return state.facet(EditorView.editable) && !state.readOnly;
}

/** Whole lines of the viewport, cut short if a pathological one is huge. */
function checkRegion(view: EditorView): { from: number; to: number } | null {
  const { doc } = view.state;
  const from = doc.lineAt(view.viewport.from).from;
  const to = doc.lineAt(view.viewport.to).to;
  if (to - from <= MAX_REGION_CHARS) return { from, to };

  let line = doc.lineAt(from);
  if (line.to - from > MAX_REGION_CHARS) return null;
  let end = line.to;
  while (line.number < doc.lines) {
    line = doc.line(line.number + 1);
    if (line.to - from > MAX_REGION_CHARS) break;
    end = line.to;
  }
  return { from, to: end };
}

class SpellcheckPlugin {
  decorations: DecorationSet = Decoration.none;
  private readonly view: EditorView;
  private readonly source: SpellSource;
  private readonly unsubscribe: () => void;
  private timer: ReturnType<typeof setTimeout> | undefined;
  /** Bumped by every `schedule`, so a pass started earlier knows it is out of date. */
  private generation = 0;
  private destroyed = false;
  /** True from an edit until the caret moves on by itself. */
  private typing = false;

  constructor(view: EditorView, source: SpellSource) {
    this.view = view;
    this.source = source;
    this.unsubscribe = source.subscribe(() => this.schedule(0));
    this.schedule(0);
  }

  update(update: ViewUpdate): void {
    if (update.docChanged) {
      this.decorations = this.decorations.map(update.changes);
      this.typing = true;
      this.schedule(TYPING_DELAY_MS);
      return;
    }
    const movedOn =
      this.typing && (update.selectionSet || (update.focusChanged && !update.view.hasFocus));
    if (movedOn) this.typing = false;
    if (
      movedOn ||
      update.viewportChanged ||
      syntaxTree(update.startState) !== syntaxTree(update.state) ||
      isWritable(update.startState) !== isWritable(update.state)
    ) {
      this.schedule(SETTLE_DELAY_MS);
    }
  }

  destroy(): void {
    this.destroyed = true;
    clearTimeout(this.timer);
    this.unsubscribe();
  }

  /** The underlined word that touches `pos`, if any. */
  wordAt(pos: number): { from: number; to: number } | null {
    const cursor = this.decorations.iter(pos);
    return cursor.value && cursor.from <= pos ? { from: cursor.from, to: cursor.to } : null;
  }

  private schedule(delay: number): void {
    if (this.destroyed) return;
    this.generation += 1;
    const generation = this.generation;
    clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = undefined;
      void this.run(generation);
    }, delay);
  }

  private isStale(generation: number): boolean {
    return this.destroyed || generation !== this.generation;
  }

  /** A failing pass must never get in the way of writing. */
  private async run(generation: number): Promise<void> {
    try {
      await this.checkViewport(generation);
    } catch (error) {
      console.warn("Spell check could not update the editor", error);
    }
  }

  private async checkViewport(generation: number): Promise<void> {
    const { view, source } = this;
    if (view.compositionStarted) {
      this.schedule(TYPING_DELAY_MS);
      return;
    }

    await source.ready();
    if (this.isStale(generation)) return;
    if (!source.isActive() || !isWritable(view.state)) {
      this.clear();
      return;
    }

    const state = view.state;
    const region = checkRegion(view);
    if (!region) return;
    // Without the syntax tree code cannot be told from prose. If the parser is
    // still busy, the update that delivers its tree runs this again.
    const tree = ensureSyntaxTree(state, region.to, PARSE_BUDGET_MS);
    if (!tree) return;

    const words = findSpellWords(
      state.sliceDoc(region.from, region.to),
      syntaxSkips(tree, region.from, region.to),
      region.from,
    );
    const misspelled = words.length
      ? await source.check(words.map((word) => word.word))
      : new Set<string>();
    if (this.isStale(generation) || view.state.doc !== state.doc) return;
    if (view.compositionStarted) {
      this.schedule(TYPING_DELAY_MS);
      return;
    }

    this.replace(
      region,
      visibleMisspellings(words, misspelled, (word) => source.isIgnored(word), this.caret()),
    );
  }

  /** The caret position while the word at it is still being typed. */
  private caret(): number | null {
    const { main } = this.view.state.selection;
    return this.typing && main.empty && this.view.hasFocus ? main.head : null;
  }

  private replace(region: { from: number; to: number }, words: readonly SpellWord[]): void {
    if (this.showsExactly(region, words)) return;
    this.decorations = this.decorations.update({
      add: words.map((word) => MISSPELLED.range(word.from, word.to)),
      filter: () => false,
      filterFrom: region.from,
      filterTo: region.to,
    });
    this.refresh();
  }

  private showsExactly(region: { from: number; to: number }, words: readonly SpellWord[]): boolean {
    let index = 0;
    let same = true;
    this.decorations.between(region.from, region.to, (from, to) => {
      const word = words[index++];
      if (!word || word.from !== from || word.to !== to) {
        same = false;
        return false;
      }
    });
    return same && index === words.length;
  }

  private clear(): void {
    if (this.decorations.size === 0) return;
    this.decorations = Decoration.none;
    this.refresh();
  }

  /**
   * Makes the editor read the decorations again. An empty update leaves update
   * listeners and other plugins alone, unlike a dispatched effect, which the
   * editors' own listeners would treat as an edit. Only ever called from a
   * timer or a promise, never from inside an update.
   */
  private refresh(): void {
    this.view.update([]);
  }
}

const theme = EditorView.baseTheme({
  [`.${MISSPELLED_CLASS}`]: {
    textDecorationLine: "underline",
    textDecorationStyle: "wavy",
    textDecorationColor: "var(--danger, #f85149)",
    textUnderlineOffset: "0.2em",
  },
});

/**
 * Underlines misspelled words as you write and reports right-clicks on them.
 *
 * Needs the markdown language in the same editor, which is how code, links and
 * the like are kept out of it.
 */
export function spellcheckExtension(options: SpellcheckOptions): Extension {
  const source = options.source ?? spellcheck;
  const plugin = ViewPlugin.define((view) => new SpellcheckPlugin(view, source), {
    decorations: (value) => value.decorations,
  });

  return [
    plugin,
    theme,
    Prec.high(
      EditorView.domEventHandlers({
        contextmenu(event, view) {
          const target = event.target;
          if (!(target instanceof Element)) return false;
          const marked = target.closest(`.${MISSPELLED_CLASS}`);
          if (!marked || !view.contentDOM.contains(marked)) return false;
          const found = view.plugin(plugin)?.wordAt(view.posAtDOM(marked));
          if (!found) return false;

          // A selection that is more than this word belongs to the editor's own menu.
          const { main } = view.state.selection;
          if (!main.empty && (main.from < found.from || main.to > found.to)) return false;

          event.preventDefault();
          event.stopPropagation();
          options.onContextMenu({
            view,
            from: found.from,
            to: found.to,
            word: view.state.sliceDoc(found.from, found.to),
            clientX: event.clientX,
            clientY: event.clientY,
          });
          return true;
        },
      }),
    ),
  ];
}
