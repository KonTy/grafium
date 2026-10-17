import { isEmptyOrderedListItem } from "./orderedList";

export function isEmptyEditorBlock(content: string): boolean {
  const normalized = content.replace(/[\u200B-\u200D\uFEFF]/g, "").trim();
  return normalized === "" || /^[-*+]$/.test(normalized) || isEmptyOrderedListItem(content);
}
