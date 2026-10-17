import { expect, it } from "vitest";
import { isEmptyEditorBlock } from "./editorEmpty";

it.each(["", " \t\n\u00a0\u200b", "-", "* ", "+\u200d", "2.", "2) ", "2.1.\t\u200b", "2.1)\u00a0"])(
  "treats %s as an editable empty outline item", (content) => expect(isEmptyEditorBlock(content)).toBe(true),
);

it.each(["2. Text", "- Text", "2.1", "1.5 metres", "```\n\n```", "2.\nKeep", "    2. "])(
  "never deletes nonempty or code content %s as an empty item", (content) => expect(isEmptyEditorBlock(content)).toBe(false),
);
