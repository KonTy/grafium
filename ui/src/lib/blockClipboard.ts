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

export async function writeClipboardText(text: string): Promise<void> {
  let clipboardError: unknown;
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return;
    }
  } catch (error) {
    // WebKitGTK can reject the async API; the user-gesture copy command still works.
    clipboardError = error;
  }
  const active = document.activeElement;
  const scratch = document.createElement("textarea");
  scratch.value = text;
  scratch.style.position = "fixed";
  scratch.style.left = "-9999px";
  scratch.style.opacity = "0";
  document.body.appendChild(scratch);
  try {
    scratch.select();
    if (!document.execCommand("copy")) throw new Error("Clipboard text API is unavailable", { cause: clipboardError });
  } finally {
    scratch.remove();
    if (active instanceof HTMLElement && active.isConnected) active.focus({ preventScroll: true });
  }
}
