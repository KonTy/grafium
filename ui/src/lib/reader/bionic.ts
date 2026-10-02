import * as CFI from "./foliate-cfi";
import { BIONIC_WORD_RE, bionicPrefixLength } from "../bionicText";

interface ResolvedCFI {
  index: number;
  anchor?: (doc: Document) => Range;
}

export interface BionicCFIView {
  book: { sections: readonly { cfi?: string | null }[] };
  getCFI(index: number, range?: Range | null): string;
  resolveCFI(cfi: string): ResolvedCFI | undefined;
}

interface Replacement {
  texts: Text[];
  first: ChildNode | null;
}

interface SelectionPoint {
  node: Node;
  offset: number;
  next?: ChildNode;
}

type Boundary = [Node, number];

const wrappers = new WeakSet<Node>();
const documents = new WeakMap<Document, HTMLElement[]>();
const installedViews = new WeakSet<BionicCFIView>();
const skipSelector = [
  "pre", "code", "kbd", "samp", "math", "svg", "script", "style", "noscript",
  "textarea", "input", "button", "select", "option", "summary", "a[href]",
  "iframe", "audio", "video", "[hidden]", "[inert]", '[aria-hidden="true"]',
  '[contenteditable]:not([contenteditable="false"])', '[role="button"]',
  ".katex", ".MathJax",
].join(",");
const cfiFilter = (node: Node): number => wrappers.has(node)
  ? NodeFilter.FILTER_SKIP : NodeFilter.FILTER_ACCEPT;

function normalizedRange(range: Range): Range {
  const normalize = (node: Node, offset: number, toEnd: boolean): Boundary => {
    if (node.nodeType !== Node.ELEMENT_NODE) return [node, offset];
    const findText = (backward: boolean): Boundary | undefined => {
      let child: ChildNode | null = node.childNodes[backward ? offset - 1 : offset] ?? null;
      while (child) {
        const walker = node.ownerDocument!.createTreeWalker(
          child, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_CDATA_SECTION,
        );
        const text = child.nodeType === Node.TEXT_NODE || child.nodeType === Node.CDATA_SECTION_NODE
          ? child : backward ? walker.lastChild() : walker.nextNode();
        if (text) return [text, backward ? (text.nodeValue?.length ?? 0) : 0];
        child = backward ? child.previousSibling : child.nextSibling;
      }
    };
    // Foliate cannot encode element child offsets. Search only the adjacent
    // subtrees, preserving existing text endpoints and their canonical CFIs.
    return findText(toEnd) ?? findText(!toEnd) ?? [node, offset];
  };
  const result = range.cloneRange();
  result.setStart(...normalize(range.startContainer, range.startOffset, false));
  if (range.collapsed) result.collapse(true);
  else result.setEnd(...normalize(range.endContainer, range.endOffset, true));
  return result;
}

/** Install before enabling Bionic; publisher elements remain part of canonical CFIs. */
export function installBionicCFI(view: BionicCFIView): void {
  if (installedViews.has(view)) return;
  installedViews.add(view);
  const originalGetCFI = view.getCFI;
  const originalResolveCFI = view.resolveCFI;
  view.getCFI = function (index, range) {
    if (!range) return originalGetCFI.call(this, index, range);
    const base = this.book.sections[index].cfi ?? CFI.fake.fromIndex(index);
    return CFI.joinIndir(base, CFI.fromRange(normalizedRange(range), cfiFilter));
  };
  view.resolveCFI = function (cfi) {
    // EPUB package assertions, not synthetic spine numbers, determine the index.
    const resolved = originalResolveCFI.call(this, cfi);
    if (!resolved?.anchor) return resolved;
    const parts = CFI.parse(cfi);
    (parts.parent ?? parts).shift();
    return { ...resolved, anchor: doc => CFI.toRange(doc, parts, cfiFilter) };
  };
}

function captureSelection(doc: Document, replacements: WeakMap<Node, Replacement>): () => void {
  const selection = doc.getSelection?.();
  if (!selection?.rangeCount || !selection.anchorNode || !selection.focusNode) return () => {};
  const capture = (node: Node, offset: number): SelectionPoint => ({
    node, offset, next: node.nodeType === 1 ? node.childNodes[offset] : undefined,
  });
  const anchor = capture(selection.anchorNode, selection.anchorOffset);
  const focus = capture(selection.focusNode, selection.focusOffset);
  const ranges = Array.from({ length: selection.rangeCount }, (_, index): [SelectionPoint, SelectionPoint] => {
    const range = selection.getRangeAt(index);
    return [capture(range.startContainer, range.startOffset),
      capture(range.endContainer, range.endOffset)];
  });
  const restore = ({ node, offset, next }: SelectionPoint): Boundary => {
    const replacement = replacements.get(node);
    if (replacement) {
      for (const text of replacement.texts) {
        if (offset <= text.length) return [text, offset];
        offset -= text.length;
      }
    }
    if (node.nodeType === 1) {
      const child = next ? replacements.get(next)?.first ?? next : undefined;
      return [node, child ? Array.from(node.childNodes).indexOf(child)
        : node.childNodes.length];
    }
    return [node, offset];
  };
  return () => {
    if (ranges.length === 1 && selection.setBaseAndExtent) {
      selection.setBaseAndExtent(...restore(anchor), ...restore(focus));
    } else {
      selection.removeAllRanges();
      for (const [start, end] of ranges) {
        const range = doc.createRange();
        range.setStart(...restore(start));
        range.setEnd(...restore(end));
        selection.addRange(range);
      }
    }
  };
}

/** Presentation only: wrappers stay in place when disabled to avoid selection churn. */
export function setBookBionic(doc: Document, enabled: boolean): void {
  let prefixes = documents.get(doc);
  if (!prefixes && !enabled) return;
  if (!prefixes) {
    prefixes = [];
    const replacements = new WeakMap<Node, Replacement>();
    const restoreSelection = captureSelection(doc, replacements);
    const root = doc.body ?? doc.documentElement;
    const skipped = new WeakMap<Element, boolean>();
    const shouldSkip = (element: Element | null): boolean => {
      if (!element) return false;
      const cached = skipped.get(element);
      if (cached !== undefined) return cached;
      const style = doc.defaultView?.getComputedStyle(element) ?? (element as HTMLElement).style;
      const skip = wrappers.has(element) || element.matches(skipSelector)
        || style?.display === "none" || style?.visibility === "hidden"
        || style?.visibility === "collapse" || shouldSkip(element.parentElement);
      skipped.set(element, skip);
      return skip;
    };
    const walker = doc.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
      acceptNode: node => node.nodeValue?.trim() && !shouldSkip(node.parentElement)
        ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT,
    });
    const nodes: Text[] = [];
    while (walker.nextNode()) nodes.push(walker.currentNode as Text);
    for (const node of nodes) {
      const fragment = doc.createDocumentFragment();
      const texts: Text[] = [];
      const appendText = (parent: DocumentFragment | HTMLElement, text: string): void => {
        if (!text) return;
        const child = doc.createTextNode(text);
        texts.push(child);
        parent.append(child);
      };
      let offset = 0;
      for (const match of node.data.matchAll(BIONIC_WORD_RE)) {
        const chars = Array.from(match[0]);
        const length = bionicPrefixLength(match[0]);
        if (length >= chars.length) continue;
        appendText(fragment, node.data.slice(offset, match.index));
        const prefix = doc.createElementNS("http://www.w3.org/1999/xhtml", "span");
        prefix.className = "grafium-book-bionic-prefix";
        wrappers.add(prefix);
        prefixes.push(prefix);
        appendText(prefix, chars.slice(0, length).join(""));
        fragment.append(prefix);
        offset = match.index + chars.slice(0, length).join("").length;
      }
      if (!offset) continue;
      appendText(fragment, node.data.slice(offset));
      replacements.set(node, { texts, first: fragment.firstChild });
      node.replaceWith(fragment);
    }
    documents.set(doc, prefixes);
    restoreSelection();
  }
  for (const prefix of prefixes) {
    prefix.style.setProperty("font-weight", enabled ? "bolder" : "inherit", "important");
  }
}
