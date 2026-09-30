import { invoke } from "@tauri-apps/api/core";

export type BookImportMode = "original" | "markdown";

export const ORIGINAL_BOOK_EXTENSIONS = ["epub", "fb2", "mobi", "azw3", "pdf"];
export const CONVERTIBLE_BOOK_EXTENSIONS = [
  ...ORIGINAL_BOOK_EXTENSIONS, "html", "htm", "xhtml", "md", "markdown", "mdown",
  "txt", "text", "azw", "azw4", "lit", "lrf", "pdb", "rb", "snb", "tcr",
  "odt", "docx", "rtf", "cbz", "cbr",
];

export function queueBookImport(mode: BookImportMode, sourcePath: string): Promise<string> {
  const path = sourcePath.trim();
  if (!path) return Promise.reject(new Error("Choose a book file or folder first."));
  return mode === "original"
    ? invoke<string>("books_import_originals", { sourcePaths: [path] })
    : invoke<string>("books_import_directory", { sourceDir: path });
}
