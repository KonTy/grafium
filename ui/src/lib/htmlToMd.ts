import TurndownService from "turndown";

const turndown = new TurndownService({
  headingStyle: "atx",
  codeBlockStyle: "fenced",
  bulletListMarker: "-",
});

// Strikethrough support
turndown.addRule("strikethrough", {
  filter: ["del", "s", "strike"],
  replacement: (content) => `~~${content}~~`,
});

// Preserve code blocks with language
turndown.addRule("fencedCodeBlock", {
  filter: (node) => {
    return node.nodeName === "PRE" && !!node.querySelector("code");
  },
  replacement: (_content, node) => {
    const code = (node as HTMLElement).querySelector("code");
    if (!code) return _content;
    const lang = (code.className.match(/language-(\S+)/) || [])[1] || "";
    const text = code.textContent || "";
    return `\n\`\`\`${lang}\n${text}\n\`\`\`\n`;
  },
});

export function htmlToMarkdown(html: string): string {
  return turndown.turndown(html).trim();
}

export interface PasteBlock {
  content: string;
  depth: number; // 0 = top level, 1 = child, 2 = grandchild, etc.
}

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

    // Plain text paragraph
    if (current.length === 0) {
      // Starting a new paragraph — flush resets
      baseIndent = -1;
      listBaseDepth = 0;
    }
    current.push(line);
    lastWasParagraph = true;
  }
  flush();

  return blocks.length > 0 ? blocks : [{ content: "", depth: 0 }];
}
