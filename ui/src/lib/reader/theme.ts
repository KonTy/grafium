import type { ReaderTheme } from "../bookReaderTheme";

const visual = "svg,math,img,picture,canvas,video";
const styles = new WeakMap<Document, HTMLStyleElement>();
interface PublisherType {
  element: HTMLElement | SVGElement;
  fontSize: number;
  lineHeight: number;
  scale: boolean;
  original: { property: string; value: string; priority: string }[];
}
const publisherType = new WeakMap<Document, PublisherType[]>();

function applyBookSize(doc: Document, size: number) {
  let typography = publisherType.get(doc);
  if (!typography) {
    // Read the complete publisher baseline before writing any ancestor: em,
    // inherited sizes, and inline !important declarations must not compound.
    typography = [doc.body, ...doc.body.querySelectorAll<HTMLElement | SVGElement>("*")]
      .filter(el => !el.parentElement?.closest(visual) && !["STYLE", "SCRIPT", "LINK"].includes(el.tagName))
      .map(element => {
        const computed = doc.defaultView!.getComputedStyle(element);
        return {
          element, fontSize: parseFloat(computed.fontSize), lineHeight: parseFloat(computed.lineHeight),
          scale: !element.matches(visual) && !!element.textContent?.trim(),
          original: ["font-size", "line-height"].map(property => ({
            property, value: element.style.getPropertyValue(property),
            priority: element.style.getPropertyPriority(property),
          })),
        };
      });
    publisherType.set(doc, typography);
  }
  for (const { element, fontSize, lineHeight, scale, original } of typography) {
    if (size === 100) {
      for (const { property, value, priority } of original) {
        if (value) element.style.setProperty(property, value, priority);
        else element.style.removeProperty(property);
      }
    } else {
      const factor = scale ? size / 100 : 1;
      if (Number.isFinite(fontSize)) element.style.setProperty("font-size", `${fontSize * factor}px`, "important");
      if (Number.isFinite(lineHeight)) element.style.setProperty("line-height", `${lineHeight * factor}px`, "important");
    }
  }
}

/** Recolor HTML prose, not pixels, SVG artwork, or image-only containers. */
export function applyBookTheme(doc: Document, theme: ReaderTheme, size = 100) {
  const win = doc.defaultView;
  if (!win) return;
  applyBookSize(doc, size);
  let style = styles.get(doc);
  if (!style) {
    // Preserve artwork's original inherited ink and backing before recoloring
    // ancestors. In particular, SVG currentColor must not become theme text.
    for (const el of doc.body.querySelectorAll<HTMLElement | SVGElement>(visual)) {
      if (el.parentElement?.closest(visual)) continue;
      const computed = win.getComputedStyle(el);
      el.style.setProperty("color", computed.color, "important");
      if (computed.backgroundColor === "rgba(0, 0, 0, 0)" || computed.backgroundColor === "transparent") {
        for (let ancestor = el.parentElement; ancestor; ancestor = ancestor.parentElement) {
          const background = win.getComputedStyle(ancestor).backgroundColor;
          if (background && background !== "rgba(0, 0, 0, 0)" && background !== "transparent") {
            el.style.setProperty("background-color", background, "important");
            break;
          }
        }
      }
    }
    style = doc.createElement("style");
    doc.head.append(style);
    styles.set(doc, style);
  }
  style.textContent = bookThemeStyles(theme);
  doc.body.style.setProperty("inline-size", "auto", "important");
  doc.body.style.setProperty("min-inline-size", "0", "important");
  for (const el of doc.body.querySelectorAll<HTMLElement>("p,div,section,article,blockquote,h1,h2,h3,h4,h5,h6")) {
    if (el.closest(visual) || /absolute|fixed/.test(win.getComputedStyle(el).position)) continue;
    el.style.setProperty("max-inline-size", "100%", "important");
    el.style.setProperty("min-inline-size", "0", "important");
    el.style.setProperty("overflow-wrap", "anywhere", "important");
  }
  for (const el of [doc.documentElement, doc.body, ...doc.body.querySelectorAll<HTMLElement>("*")]) {
    if (el.closest(visual) || el.namespaceURI !== "http://www.w3.org/1999/xhtml"
      || ["STYLE", "SCRIPT", "LINK"].includes(el.tagName)) continue;
    const page = el === doc.documentElement || el === doc.body;
    if (!page && !el.textContent.trim() && el.querySelector(visual)) continue;
    const image = win.getComputedStyle(el).backgroundImage;
    el.style.setProperty("color", el.closest("a[href]") ? theme.link : theme.text, "important");
    el.style.setProperty("-webkit-text-fill-color", "currentColor", "important");
    el.style.setProperty("text-shadow", "none", "important");
    if (page || !image || image === "none")
      el.style.setProperty("background-color", page ? theme.background : "transparent", "important");
  }
}

export function bookThemeStyles(theme: ReaderTheme) {
  return `html{color:${theme.text};background-color:${theme.background}}
    ::selection{background-color:${theme.selectionBackground} !important;color:${theme.selectionText} !important}`;
}
