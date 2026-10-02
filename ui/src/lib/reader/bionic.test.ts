import { describe, expect, it, vi } from "vitest";
import * as CFI from "./foliate-cfi";
import { installBionicCFI, setBookBionic } from "./bionic";

const prefixSelector = ".grafium-book-bionic-prefix";

function bookDocument(html: string) {
  const doc = document.implementation.createHTMLDocument("Synthetic book");
  doc.body.innerHTML = html;
  return doc;
}

function makeView(cfi?: string) {
  return {
    book: { sections: [{ cfi }] },
    getCFI(index: number, range?: Range) {
      const base = this.book.sections[index].cfi ?? CFI.fake.fromIndex(index);
      return range ? CFI.joinIndir(base, CFI.fromRange(range)) : base;
    },
    resolveCFI(cfi: string) {
      const parts = CFI.parse(cfi);
      const index = CFI.fake.toIndex((parts.parent ?? parts).shift());
      return { index, anchor: (doc: Document) => CFI.toRange(doc, parts) };
    },
  };
}

function textPoint(root: Node, offset: number): [Node, number] {
  const walker = root.ownerDocument!.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  while (walker.nextNode()) {
    const node = walker.currentNode as Text;
    if (offset <= node.length) return [node, offset];
    offset -= node.length;
  }
  throw new Error("Invalid synthetic text offset");
}

function textRange(root: Node, start: number, end: number) {
  const range = root.ownerDocument!.createRange();
  range.setStart(...textPoint(root, start));
  range.setEnd(...textPoint(root, end));
  return range;
}

describe("book Bionic presentation and canonical CFIs", () => {
  it("keeps exact before/on/off CFIs and quotes across Unicode and publisher elements", () => {
    const html = '<p>Reading 𐐀𐐨𐐲𐐼 élan 中文文字 <em>faster</em> ' +
      '<span class="grafium-book-bionic-prefix bionic-word" data-bionic-word="1">' +
      'publisher</span><img alt="photo"> ending.</p>';
    const pristine = bookDocument(html);
    const doc = bookDocument(html);
    const root = doc.querySelector("p")!;
    const originalText = root.textContent!;
    const view = makeView();
    const examples = Array.from({ length: originalText.length + 1 }, (_, start) => {
      const end = Math.min(originalText.length, start + 9);
      const range = textRange(root, start, end);
      return { start, end, cfi: view.getCFI(0, range), quote: range.toString() };
    });
    installBionicCFI(view);
    for (const enabled of [false, true, true, false, true, false]) {
      setBookBionic(doc, enabled);
      expect(root.textContent).toBe(originalText);
      for (const { start, end, cfi, quote } of examples) {
        const range = textRange(root, start, end);
        expect(view.getCFI(0, range)).toBe(cfi);
        expect(view.resolveCFI(cfi).anchor(doc).toString()).toBe(quote);
        expect(view.resolveCFI(view.getCFI(0, range)).anchor(pristine).toString()).toBe(quote);
      }
    }
    expect(doc.querySelector("img")?.getAttribute("alt")).toBe("photo");
  });

  it("uses the same short-word, combining-mark and surrogate-aware prefixes as notes", () => {
    const doc = bookDocument("<p>I to reading 𐐀𐐨𐐲𐐼 élan 12345 one-two don’t</p>");
    setBookBionic(doc, true);
    expect(Array.from(doc.querySelectorAll(prefixSelector), node => node.textContent))
      .toEqual(["t", "rea", "𐐀𐐨", "él", "123", "one", "don"]);
    expect(doc.body.textContent).toBe("I to reading 𐐀𐐨𐐲𐐼 élan 12345 one-two don’t");
  });

  it("never ignores publisher-controlled lookalike wrappers", () => {
    const doc = bookDocument('<p>Before <span class="grafium-book-bionic-prefix">' +
      '<strong class="bionic-word-prefix">Publisher</strong></span> after</p>');
    const publisher = doc.querySelector("strong")!;
    const view = makeView();
    const original = view.getCFI(0, textRange(publisher, 1, 8));
    installBionicCFI(view);
    setBookBionic(doc, true);
    expect(view.getCFI(0, textRange(publisher, 1, 8))).toBe(original);
    expect(view.resolveCFI(original).anchor(doc).toString()).toBe("ublishe");
    expect(publisher.parentElement!.getAttribute("style")).toBeNull();
  });

  it("canonicalizes range endpoints on its own prefix elements", () => {
    const doc = bookDocument("<p>Reading faster</p>");
    const view = makeView();
    const expected = view.getCFI(0, textRange(doc.querySelector("p")!, 0, 3));
    installBionicCFI(view);
    setBookBionic(doc, true);
    const range = doc.createRange();
    range.selectNodeContents(doc.querySelector(prefixSelector)!);
    expect(view.getCFI(0, range)).toBe(expected);
    range.collapse(false);
    expect(view.getCFI(0, range)).toBe(
      view.getCFI(0, textRange(doc.querySelector("p")!, 3, 3)),
    );
  });

  it("merges adjacent original text nodes using canonical UTF-16 offsets", () => {
    const doc = bookDocument("<p></p>");
    const p = doc.querySelector("p")!;
    p.append("Reading ", "𐐀𐐨𐐲𐐼 faster", "", " ending");
    const view = makeView();
    const cfi = view.getCFI(0, textRange(p, 10, 19));
    installBionicCFI(view);
    setBookBionic(doc, true);
    expect(view.getCFI(0, textRange(p, 10, 19))).toBe(cfi);
    expect(view.resolveCFI(cfi).anchor(doc).toString()).toBe(p.textContent!.slice(10, 19));
  });

  it("canonicalizes actual selectNodeContents paragraph ranges before/on/off", () => {
    const html = '<p id="passage">Reading <em>𐐀𐐨𐐲𐐼 faster</em> together.</p>';
    const doc = bookDocument(html);
    const pristine = bookDocument(html);
    const p = doc.querySelector("p")!;
    const quote = p.textContent!;
    const view = makeView();
    const expected = view.getCFI(0, textRange(p, 0, quote.length));
    installBionicCFI(view);
    for (const enabled of [false, true, false, true]) {
      setBookBionic(doc, enabled);
      const range = doc.createRange();
      range.selectNodeContents(p);
      const cfi = view.getCFI(0, range);
      expect(cfi).toBe(expected);
      expect(view.resolveCFI(cfi).anchor(doc).toString()).toBe(quote);
      expect(view.resolveCFI(cfi).anchor(pristine).toString()).toBe(quote);
      expect(range.startContainer).toBe(p);
      expect(range.endContainer).toBe(p);
      expect(range.endOffset).toBe(p.childNodes.length);
    }
  });

  it("normalizes cross-node element offsets to the adjacent nested text endpoints", () => {
    const html = '<div><p id="first"><span>Before </span><em>Reading</em> faster</p>' +
      '<p id="second"><strong>Second <i>nested</i></strong><span> suffix</span></p></div>';
    const doc = bookDocument(html);
    const pristine = bookDocument(html);
    const first = doc.getElementById("first")!;
    const second = doc.getElementById("second")!;
    const view = makeView();
    const text = doc.createRange();
    text.setStart(first.querySelector("em")!.firstChild!, 0);
    text.setEnd(second.querySelector("i")!.firstChild!, 6);
    const expected = view.getCFI(0, text);
    const quote = text.toString();
    installBionicCFI(view);
    for (const enabled of [false, true, false, true]) {
      setBookBionic(doc, enabled);
      const range = doc.createRange();
      range.setStart(first, 1);
      range.setEnd(second, 1);
      const cfi = view.getCFI(0, range);
      expect(cfi).toBe(expected);
      expect(view.resolveCFI(cfi).anchor(doc).toString()).toBe(quote);
      expect(view.resolveCFI(cfi).anchor(pristine).toString()).toBe(quote);
    }
  });

  it("preserves collapsed element boundaries, including the end of a paragraph", () => {
    const doc = bookDocument("<p><span>Reading</span><em> faster</em></p>");
    const p = doc.querySelector("p")!;
    const view = makeView();
    installBionicCFI(view);
    for (const enabled of [false, true, false]) {
      setBookBionic(doc, enabled);
      for (const offset of [0, 1, 2]) {
        const range = doc.createRange();
        range.setStart(p, offset);
        range.collapse(true);
        const resolved = view.resolveCFI(view.getCFI(0, range)).anchor(doc);
        expect(resolved.collapsed).toBe(true);
        const preceding = doc.createRange();
        preceding.selectNodeContents(p);
        preceding.setEnd(resolved.startContainer, resolved.startOffset);
        expect(preceding.toString()).toBe(["", "Reading", "Reading faster"][offset]);
      }
    }
  });

  it("preserves EPUB package-resolved section indices instead of guessing them", () => {
    const doc = bookDocument("<p>Reading faster</p>");
    const view = makeView("epubcfi(/6/18[chapter-nine])");
    const originalResolve = vi.fn((cfi: string) => {
      const result = makeView().resolveCFI(cfi);
      return { ...result, index: 3 };
    });
    view.resolveCFI = originalResolve;
    const cfi = view.getCFI(0, textRange(doc.querySelector("p")!, 2, 10));
    installBionicCFI(view);
    setBookBionic(doc, true);
    expect(view.resolveCFI(cfi).index).toBe(3);
    expect(view.resolveCFI(cfi).anchor(doc).toString()).toBe("ading fa");
    expect(originalResolve).toHaveBeenCalledWith(cfi);
    expect(view.getCFI(0)).toBe("epubcfi(/6/18[chapter-nine])");
  });

  it("installs once and toggles weight without replacing nodes or adding stylesheets", () => {
    const doc = bookDocument("<p>Reading faster</p>");
    const head = doc.head.innerHTML;
    const view = makeView();
    installBionicCFI(view);
    const get = view.getCFI;
    const resolve = view.resolveCFI;
    installBionicCFI(view);
    expect(view.getCFI).toBe(get);
    expect(view.resolveCFI).toBe(resolve);
    setBookBionic(doc, false);
    expect(doc.querySelector(prefixSelector)).toBeNull();
    setBookBionic(doc, true);
    const nodes = Array.from(doc.querySelectorAll<HTMLElement>(prefixSelector));
    for (const enabled of [true, false, false, true]) {
      setBookBionic(doc, enabled);
      expect(Array.from(doc.querySelectorAll(prefixSelector))).toEqual(nodes);
      for (const node of nodes) {
        expect(node.style.fontWeight).toBe(enabled ? "bolder" : "inherit");
        expect(node.style.length).toBe(1);
      }
    }
    expect(doc.head.innerHTML).toBe(head);
  });

  it("leaves code, math, interactive, hidden, and non-prose subtrees untouched", () => {
    const doc = bookDocument('<p>Readable words</p><pre>Code words</pre><code>Inline code</code>' +
      '<math><mi>Mathematics</mi></math><svg><text>Vector text</text></svg>' +
      '<script>Script words</script><style>.hidden { display: none; }</style>' +
      '<button>Button words</button><a href="#x">Link words</a><textarea>Input words</textarea>' +
      '<div hidden>Hidden words</div><div aria-hidden="true">Hidden words</div>' +
      '<div style="display:none">Hidden words</div><div style="visibility:hidden">Hidden words</div>' +
      '<div contenteditable="true">Editing words</div><div inert>Inert words</div>' +
      '<div class="katex">Math words</div>');
    const html = Array.from(doc.body.children).slice(1).map(node => node.outerHTML);
    setBookBionic(doc, true);
    expect(doc.querySelectorAll(prefixSelector)).toHaveLength(2);
    expect(Array.from(doc.body.children).slice(1).map(node => node.outerHTML)).toEqual(html);
  });

  it("respects computed hidden styles in a live document", () => {
    document.body.innerHTML = '<style>.hidden { display: none }</style>' +
      '<div class="hidden"><p>Hidden words</p></div><p>Visible words</p>';
    setBookBionic(document, true);
    expect(document.querySelector(".hidden")!.querySelector(prefixSelector)).toBeNull();
    expect(document.querySelectorAll(prefixSelector)).toHaveLength(2);
  });

  it.each(["forward", "backward", "element", "caret"])(
    "preserves %s selections across first application and repeated toggles", direction => {
      const frame = document.createElement("iframe");
      document.body.append(frame);
      const doc = frame.contentDocument!;
      doc.body.innerHTML = "<p>Reading 𐐀𐐨𐐲𐐼 faster</p>";
      const p = doc.querySelector("p")!;
      const selection = doc.getSelection()!;
      const start = textPoint(p, 2);
      const end = textPoint(p, direction === "caret" ? 2 : 18);
      if (direction === "element") selection.setBaseAndExtent(p, 0, p, 1);
      else if (direction === "backward") selection.setBaseAndExtent(...end, ...start);
      else selection.setBaseAndExtent(...start, ...end);
      const quote = selection.toString();
      for (const enabled of [true, true, false, true]) {
        setBookBionic(doc, enabled);
        expect(selection.toString()).toBe(quote);
        expect(selection.rangeCount).toBe(1);
        expect(selection.isCollapsed).toBe(direction === "caret");
        if (direction === "backward") {
          const range = selection.getRangeAt(0);
          expect(selection.anchorNode).toBe(range.endContainer);
          expect(selection.anchorOffset).toBe(range.endOffset);
        }
      }
      frame.remove();
    },
  );

  it("works on XHTML spine documents without changing namespaces or text", () => {
    const doc = new DOMParser().parseFromString(
      '<html xmlns="http://www.w3.org/1999/xhtml"><head/><body><p>Reading faster</p></body></html>',
      "application/xhtml+xml",
    );
    const view = makeView();
    const cfi = view.getCFI(0, textRange(doc.querySelector("p")!, 1, 9));
    installBionicCFI(view);
    setBookBionic(doc, true);
    expect(doc.querySelector(prefixSelector)!.namespaceURI).toBe("http://www.w3.org/1999/xhtml");
    expect(view.getCFI(0, textRange(doc.querySelector("p")!, 1, 9))).toBe(cfi);
    expect(view.resolveCFI(cfi).anchor(doc).toString()).toBe("eading f");
  });
});
