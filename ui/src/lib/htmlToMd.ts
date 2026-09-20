import TurndownService from "turndown";

type TagName = keyof HTMLElementTagNameMap;

const turndown = new TurndownService({
  headingStyle: "atx",
  codeBlockStyle: "fenced",
  bulletListMarker: "-",
});

// Strikethrough support
turndown.addRule("strikethrough", {
  // `strike` is a valid (deprecated) tag Turndown handles at runtime, but it is
  // absent from TypeScript's HTMLElementTagNameMap, so the list needs a cast.
  filter: ["del", "s", "strike"] as unknown as TagName[],
  replacement: (content) => `~~${content}~~`,
});

function elementWithClass(node: Node, className: string): HTMLElement | null {
  if (node.nodeType !== 1) return null;
  const element = node as HTMLElement;
  return element.classList?.contains(className) ? element : null;
}

function safeWikiTarget(value: string | null, fallback: string): string | null {
  const target = (value || fallback).trim().replace(/^\[\[|\]\]$/g, "");
  if (!target || /[\u0000-\u001f\u007f<>\[\]|\n\r]/.test(target)) return null;
  return target.replace(/\\/g, "/");
}

function safeTagTarget(value: string | null, fallback: string): string | null {
  const tag = (value || fallback).trim().replace(/^#/, "").replace(/\\/g, "/");
  return /^[\p{L}\p{N}][\p{L}\p{N}\p{M}_/-]*$/u.test(tag) ? tag : null;
}

function safeBlockRef(value: string | null, fallback: string): string | null {
  const ref = (value || fallback).trim().replace(/^\(\(|\)\)$/g, "");
  return /^[a-f0-9][a-f0-9-]{6,}$/i.test(ref) ? ref : null;
}

function safeTaskState(value: string | null, fallback: string): string | null {
  const state = (value || fallback).trim().toUpperCase();
  return /^(TODO|DOING|DONE|LATER|NOW|CANCELED|CANCELLED)$/.test(state) ? state : null;
}

function safeCodeLanguage(value: string): string {
  const lang = value.trim();
  return /^[a-zA-Z0-9_+.-]{0,64}$/.test(lang) ? lang : "";
}

turndown.addRule("grafiumPageLink", {
  filter: (node) => elementWithClass(node, "page-link") !== null,
  replacement: (content, node) => {
    const element = node as HTMLElement;
    const target = safeWikiTarget(element.getAttribute("data-page"), content);
    if (!target) return content;
    const label = element.textContent?.trim() ?? "";
    if (!label || label === target) return `[[${target}]]`;
    if (/[\u0000-\u001f\u007f<>\[\]\n\r]/.test(label)) return content;
    return `[[${target}|${label}]]`;
  },
});

turndown.addRule("grafiumTag", {
  filter: (node) => elementWithClass(node, "tag") !== null,
  replacement: (content, node) => {
    const element = node as HTMLElement;
    const tag = safeTagTarget(element.getAttribute("data-tag"), content);
    return tag ? `#${tag}` : content;
  },
});

turndown.addRule("grafiumBlockRef", {
  filter: (node) => elementWithClass(node, "block-ref") !== null,
  replacement: (content, node) => {
    const element = node as HTMLElement;
    const ref = safeBlockRef(element.getAttribute("data-ref"), content);
    return ref ? `((${ref}))` : content;
  },
});

turndown.addRule("grafiumTaskCheckbox", {
  filter: (node) => elementWithClass(node, "task-checkbox") !== null,
  replacement: () => "",
});

turndown.addRule("grafiumTaskMarker", {
  filter: (node) => elementWithClass(node, "task-marker") !== null,
  replacement: (content, node) => {
    const element = node as HTMLElement;
    const state = safeTaskState(element.getAttribute("data-task-state"), content);
    return state ? `${state} ` : content;
  },
});

// Preserve code blocks with language
turndown.addRule("fencedCodeBlock", {
  filter: (node) => {
    return node.nodeName === "PRE" && !!node.querySelector("code");
  },
  replacement: (_content, node) => {
    const code = (node as HTMLElement).querySelector("code");
    if (!code) return _content;
    const lang = safeCodeLanguage((code.className.match(/language-(\S+)/) || [])[1] || "");
    const text = code.textContent || "";
    return `\n\`\`\`${lang}\n${text}\n\`\`\`\n`;
  },
});

function tableCellMarkdown(cell: Element): string {
  return turndown.turndown(cell.innerHTML)
    .trim()
    .replace(/[ \t]*\r?\n+[ \t]*/g, "<br>")
    .replace(/\\/g, "\\\\")
    .replace(/\|/g, "\\|");
}

function tableRowMarkdown(cells: readonly string[]): string {
  return `| ${cells.join(" | ")} |`;
}

function expandedCells(cells: readonly Element[]): string[] {
  return cells.flatMap((cell) => {
    const content = tableCellMarkdown(cell);
    const rawSpan = cell.getAttribute("colspan") ?? cell.getAttribute("aria-colspan");
    const span = Math.max(1, Number.parseInt(rawSpan ?? "1", 10) || 1);
    return [content, ...Array.from({ length: span - 1 }, () => "")];
  });
}

function nativeTableRows(table: HTMLTableElement): string[][] {
  return Array.from(table.rows).map((row) => expandedCells(Array.from(row.cells)));
}

function ariaTableRows(table: Element): string[][] {
  return Array.from(table.querySelectorAll('[role="row"]'))
    .filter((row) => row.closest('[role="table"], [role="grid"]') === table)
    .map((row) => expandedCells(Array.from(
      row.querySelectorAll(':scope > [role="columnheader"], :scope > [role="rowheader"], :scope > [role="cell"], :scope > [role="gridcell"]'),
    )));
}

turndown.addRule("table", {
  filter: (node) => node.nodeName === "TABLE"
    || (node.nodeType === 1 && ["table", "grid"].includes((node as Element).getAttribute("role") ?? "")),
  replacement: (_content, node) => {
    const element = node as Element;
    const rows = (element.nodeName === "TABLE"
      ? nativeTableRows(element as HTMLTableElement)
      : ariaTableRows(element))
      .filter((cells) => cells.length > 0);
    if (rows.length === 0) return "";

    const columnCount = Math.max(...rows.map((cells) => cells.length));
    const normalized = rows.map((cells) => [
      ...cells,
      ...Array.from({ length: columnCount - cells.length }, () => ""),
    ]);
    const delimiter = Array.from({ length: columnCount }, () => "---");
    return `\n\n${[
      tableRowMarkdown(normalized[0]),
      tableRowMarkdown(delimiter),
      ...normalized.slice(1).map(tableRowMarkdown),
    ].join("\n")}\n\n`;
  },
});

export function htmlToMarkdown(html: string): string {
  return turndown.turndown(html).trim();
}

export function htmlContainsTable(html: string): boolean {
  if (!html.trim()) return false;
  const document = new DOMParser().parseFromString(html, "text/html");
  return document.querySelector('table, [role="table"], [role="grid"]') !== null;
}

export function clipboardImageFile(data: DataTransfer | null): File | null {
  if (!data) return null;
  for (const item of Array.from(data.items)) {
    if (item.kind !== "file" || !item.type.toLowerCase().startsWith("image/")) continue;
    const file = item.getAsFile();
    if (file) return file;
  }
  return Array.from(data.files).find((file) => file.type.toLowerCase().startsWith("image/")) ?? null;
}

export function clipboardImageMarkdown(path: string, fileName: string): string {
  const name = fileName.replace(/\.[^.]+$/, "").replace(/[\[\]\\\r\n]/g, " ").trim();
  return `![${name || "Pasted image"}](${path})`;
}

/**
 * Find all remote image URLs in markdown and download them to assets/.
 * Rewrites the markdown to use local paths.
 */
export async function localizeImages(md: string, downloadFn: (url: string) => Promise<string>): Promise<string> {
  const imageRe = /!\[([^\]]*)\]\((https?:\/\/[^)]+)\)/g;
  const matches = [...md.matchAll(imageRe)];
  if (matches.length === 0) return md;

  let result = md;
  for (const match of matches) {
    const fullMatch = match[0];
    const alt = match[1];
    const url = match[2];
    try {
      const localPath = await downloadFn(url);
      result = result.replace(fullMatch, `![${alt}](${localPath})`);
    } catch (e) {
      // If download fails, keep the original URL
      console.warn(`[assets] Failed to download ${url}:`, e);
    }
  }
  return result;
}

export interface PasteBlock {
  content: string;
  depth: number; // 0 = top level, 1 = child, 2 = grandchild, etc.
}

const TASK_LINE_RE = /^(?:TODO|DOING|DONE|LATER|NOW|CANCELED|CANCELLED)\b/i;

/**
 * Split markdown into logical blocks with depth info for multi-block paste.
 * Nested list items become children. List items following a paragraph
 * become children of that paragraph.
 */
export function splitMarkdownIntoBlocks(md: string): PasteBlock[] {
  const lines = md.split("\n");
  const blocks: PasteBlock[] = [];
  let current: string[] = [];
  let currentDepth = 0;
  let inCodeFence = false;
  let baseIndent = -1;
  let lastWasParagraph = false; // track if last flushed block was a plain paragraph
  let listBaseDepth = 0; // depth offset for list items following a paragraph

  function flush() {
    const text = current.join("\n").trim();
    if (text) blocks.push({ content: text, depth: currentDepth });
    current = [];
  }

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const trimmed = line.trimStart();
    const indent = line.length - line.trimStart().length;

    // Track code fences — keep them as one block
    if (trimmed.startsWith("```")) {
      if (!inCodeFence) {
        flush();
        inCodeFence = true;
        currentDepth = 0;
        current.push(line);
        lastWasParagraph = false;
      } else {
        current.push(line);
        inCodeFence = false;
        flush();
        lastWasParagraph = false;
      }
      continue;
    }
    if (inCodeFence) {
      current.push(line);
      continue;
    }

    // Blank line = paragraph break
    if (trimmed === "") {
      flush();
      currentDepth = 0;
      // Don't reset lastWasParagraph — a blank line between paragraph and list is normal
      continue;
    }

    // Heading = new block at depth 0
    if (/^#{1,6}\s/.test(trimmed)) {
      flush();
      blocks.push({ content: trimmed, depth: 0 });
      currentDepth = 0;
      lastWasParagraph = false;
      baseIndent = -1;
      listBaseDepth = 0;
      continue;
    }

    // List item — detect depth from indentation
    const listMatch = trimmed.match(/^(?:[-*+]|\d+\.)\s+(.*)/);
    if (listMatch) {
      flush();
      if (baseIndent < 0) {
        baseIndent = indent;
        // If a paragraph preceded this list, items are children (depth +1)
        listBaseDepth = lastWasParagraph ? 1 : 0;
      }
      const relIndent = Math.max(0, indent - baseIndent);
      // 4 spaces = one nesting level (Turndown default), but also handle 2-space
      const indentDepth = relIndent >= 4 ? Math.round(relIndent / 4) : (relIndent >= 2 ? 1 : 0);
      currentDepth = listBaseDepth + indentDepth;
      current.push(listMatch[1]);
      lastWasParagraph = false;
      continue;
    }

    if (TASK_LINE_RE.test(trimmed)) {
      flush();
      baseIndent = -1;
      listBaseDepth = 0;
      currentDepth = 0;
      current.push(trimmed);
      lastWasParagraph = false;
      continue;
    }

    // Plain text paragraph
    let paragraphLine = line;
    if (current.length > 0 && baseIndent >= 0) {
      const relativeDepth = Math.max(0, currentDepth - listBaseDepth);
      const continuationIndent = baseIndent + relativeDepth * 2 + 2;
      if (indent >= continuationIndent) {
        paragraphLine = line.slice(continuationIndent);
      }
    }
    if (current.length === 0) {
      // Starting a new paragraph — flush resets
      baseIndent = -1;
      listBaseDepth = 0;
    }
    current.push(paragraphLine);
    lastWasParagraph = true;
  }
  flush();

  return blocks.length > 0 ? blocks : [{ content: "", depth: 0 }];
}
