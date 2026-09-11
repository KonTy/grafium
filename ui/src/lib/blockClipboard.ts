export interface ClipboardBlock {
  content: string;
  depth: number;
}

export function formatBlocksAsOutlineMarkdown(blocks: readonly ClipboardBlock[]): string {
  if (blocks.length === 0) return "";

  const minDepth = Math.min(...blocks.map((block) => Math.max(0, block.depth)));
  return blocks
    .map((block) => {
      const depth = Math.max(0, block.depth - minDepth);
      const indent = "  ".repeat(depth);
      const continuationIndent = `${indent}  `;
      const lines = block.content.trimEnd().split("\n");
      const firstLine = lines[0] ?? "";
      return [
        `${indent}- ${firstLine}`,
        ...lines.slice(1).map((line) => `${continuationIndent}${line}`),
      ].join("\n");
    })
    .join("\n")
    .trimEnd();
}

export function formatBlocksAsPlainText(blocks: readonly ClipboardBlock[]): string {
  return blocks
    .map((block) => block.content.trimEnd())
    .join("\n")
    .trim();
}
