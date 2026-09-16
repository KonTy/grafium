export interface RenderedReadingText {
  text: string;
  /** Map a DOM Range endpoint into the same logical text used by quotes and anchors. */
  offsetAt(node: Node, offset: number): number;
}

interface NodeOffsets {
  from: number;
  to: number;
  children?: number[];
}

const READING_CHROME = ".code-lang, .code-toolbar, .callout-title, .reading-note-ref, button, [role=toolbar], [aria-hidden=true], [hidden], script, style, template";
const BLOCK_BOUNDARIES = new Set(["P", "H1", "H2", "H3", "H4", "H5", "H6", "LI", "PRE", "TD", "TH", "TR"]);

/**
 * The renderer uses <br> for prose breaks and adjacent .code-line spans for
 * fenced code. Neither contributes separators to Range.toString/textContent.
 * Walk that actual DOM once, preserving logical breaks and omitting generated
 * labels; quotes and prefix/suffix offsets must never use different text models.
 * Native reading_scope normalizes these breaks as whitespace when validating.
 */
export function renderedReadingText(root: Element): RenderedReadingText {
  let text = "";
  let contentEnd = 0;
  const offsets = new WeakMap<Node, NodeOffsets>();
  function boundary() {
    if (text && !text.endsWith("\n")) text += "\n";
  }
  function visit(node: Node, inPre = false) {
    const element = node instanceof Element ? node : null;
    if (element?.matches(READING_CHROME)) {
      offsets.set(node, { from: text.length, to: text.length });
      return;
    }
    if (node.nodeType === Node.TEXT_NODE) {
      const from = text.length;
      const value = node.textContent ?? "";
      // Newlines between rendered block tags are HTML formatting, not content.
      if (inPre || !/^[\t \r\n]*[\r\n][\t \r\n]*$/.test(value)) {
        text += value;
        if (value) contentEnd = text.length;
      }
      offsets.set(node, { from, to: text.length });
      return;
    }
    if (!element) {
      offsets.set(node, { from: text.length, to: text.length });
      return;
    }
    const block = BLOCK_BOUNDARIES.has(element.tagName);
    if (block) boundary();
    if (element.classList.contains("code-line") && element.previousElementSibling?.classList.contains("code-line")) {
      text += "\n";
      contentEnd = text.length;
    }
    const from = text.length;
    const children = [from];
    if (element.tagName === "BR" || element.tagName === "HR") {
      text += "\n";
      contentEnd = text.length;
    } else {
      for (const child of element.childNodes) {
        visit(child, inPre || element.tagName === "PRE");
        children.push(text.length);
      }
    }
    offsets.set(node, { from, to: text.length, children });
    if (block) boundary();
  }
  visit(root);
  // Only remove trailing synthetic block boundaries, never selected code spaces.
  text = text.slice(0, contentEnd);
  function offsetAt(node: Node, offset: number): number {
    const mapped = offsets.get(node);
    if (!mapped) {
      // Endpoints inside ignored toolbar/label descendants map to zero content.
      return node.parentNode ? offsetAt(node.parentNode, 0) : 0;
    }
    const point = mapped.children
      ? mapped.children[Math.max(0, Math.min(offset, mapped.children.length - 1))]
      : Math.min(mapped.to, mapped.from + Math.max(0, offset));
    return Math.min(text.length, point);
  }
  return { text, offsetAt };
}
