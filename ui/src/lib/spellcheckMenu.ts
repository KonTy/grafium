/**
 * The spelling part of the context menu: what a right-click on an underlined
 * word offers. The editor's spell checker contributes it as a provider, and
 * the menu host shows it above the clipboard actions.
 */
import type { EditorView } from "@codemirror/view";
import {
  SPELLING_SECTION_ORDER,
  type ContextMenuAction,
  type ContextMenuItem,
  type ContextMenuSection,
} from "./contextMenuSections";
import { spellcheck } from "./spellcheck";
import { describeError, showToast } from "./toast.svelte";

/** What the menu needs from the spell checker; `SpellcheckService` provides it. */
export interface SpellMenuSource {
  suggest(word: string): Promise<string[]>;
  addWord(word: string): Promise<unknown>;
  ignore(word: string): void;
}

/** An underlined word in an editor. */
export interface MisspelledWord {
  view: EditorView;
  /** Where the word starts and ends in the document. */
  from: number;
  to: number;
  word: string;
}

/** The sections for a right-click on `target`: suggestions first, then the dictionary choices. */
export function buildSpellSections(
  target: MisspelledWord,
  source: SpellMenuSource = spellcheck,
): ContextMenuSection[] {
  const { view, from, to, word } = target;

  function replaceWith(replacement: string): void {
    view.focus();
    if (view.state.sliceDoc(from, to) !== word) {
      showToast("The text changed, so the correction was not applied.", "info");
      return;
    }
    view.dispatch({
      changes: { from, to, insert: replacement },
      selection: { anchor: from + replacement.length },
      userEvent: "input.spellcheck",
    });
  }

  async function addToDictionary(): Promise<void> {
    view.focus();
    try {
      await source.addWord(word);
    } catch (error) {
      showToast(`Could not add "${word}" to the dictionary: ${describeError(error)}`);
    }
  }

  function ignoreForNow(): void {
    source.ignore(word);
    view.focus();
  }

  async function suggestions(): Promise<ContextMenuItem[]> {
    const found = await source.suggest(word);
    if (found.length === 0) return [{ kind: "status", id: "spelling.none", label: "No suggestions" }];
    return found.map(
      (suggestion): ContextMenuAction => ({
        id: `spelling.replace:${suggestion}`,
        label: suggestion,
        emphasis: true,
        onSelect: () => replaceWith(suggestion),
      }),
    );
  }

  return [
    {
      id: "spelling",
      ariaLabel: "Spelling suggestions",
      order: SPELLING_SECTION_ORDER,
      items: [{ kind: "status", id: "spelling.loading", label: "Looking for suggestions…" }],
      resolve: suggestions,
      errorLabel: "Suggestions unavailable",
    },
    {
      id: "spelling-actions",
      ariaLabel: "Dictionary",
      order: SPELLING_SECTION_ORDER + 1,
      items: [
        { id: "spelling.add", label: "Add to dictionary", onSelect: addToDictionary },
        { id: "spelling.ignore", label: "Ignore for now", onSelect: ignoreForNow },
      ],
    },
  ];
}
