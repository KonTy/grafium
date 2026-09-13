import type { CompletionContext, CompletionResult } from "@codemirror/autocomplete";
import { EditorSelection } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { emojiIconMenuBeforeCursor } from "./emojiIconPicker";

export function emojiIconCompletionSource(context: CompletionContext): CompletionResult | null {
  const line = context.state.doc.lineAt(context.pos);
  const beforeCursor = line.text.slice(0, context.pos - line.from);
  const menu = emojiIconMenuBeforeCursor(beforeCursor);
  if (!menu || menu.entries.length === 0) return null;

  return {
    from: line.from + menu.from,
    filter: false,
    options: menu.entries.map((entry) => ({
      label: `${entry.preview} ${entry.name}`,
      detail: entry.kind === "emoji" ? `Emoji · ${entry.group}` : `Icon · ${entry.group}`,
      type: entry.kind === "emoji" ? "constant" : "keyword",
      boost: entry.kind === "emoji" ? 1 : 0,
      apply: (view: EditorView, _completion: unknown, from: number, to: number) => {
        view.dispatch({
          changes: { from, to, insert: entry.insert },
          selection: EditorSelection.cursor(from + entry.insert.length),
        });
      },
    })),
  };
}
