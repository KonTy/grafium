import { View } from "foliate-js/view.js";
import { EPUB } from "foliate-js/epub.js";
import { makeFB2 } from "foliate-js/fb2.js";
import { MOBI } from "foliate-js/mobi.js";
import { Overlayer } from "foliate-js/overlayer.js";
import { ZipReader, BlobReader, TextWriter, BlobWriter, configure } from "@zip.js/zip.js";
import { unzlibSync } from "fflate";
import { sha1 } from "@noble/hashes/sha1";
import * as pdfjs from "pdfjs-dist/build/pdf.mjs";
import { sanitizeBookDocument, localBookLink } from "../bookReaderSecurity";
import { BOOK_RENDERER_VERSION, isBookLocation } from "../bookLocations";
import { DEFAULT_READER_THEME, isReaderTheme } from "../bookReaderTheme";
import { applyBookTheme, bookThemeStyles } from "./theme";
import { installReaderInteractions } from "./interactions";
import { setReaderShortcutBindings } from "../readerHotkeys";
import { installBionicCFI, setBookBionic } from "./bionic";

const token = globalThis.GRAFIUM_BOOK_TOKEN;
delete globalThis.GRAFIUM_BOOK_TOKEN;
let theme = isReaderTheme(globalThis.GRAFIUM_BOOK_THEME) ? globalThis.GRAFIUM_BOOK_THEME : DEFAULT_READER_THEME;
delete globalThis.GRAFIUM_BOOK_THEME;
const root = document.getElementById("reader");
const send = (type, data = {}) => parent.postMessage({ channel: "grafium-book", token, type, ...data }, "*");
const error = e => send("error", { message: e instanceof Error ? e.message : String(e) });
const urls = new Set();
const blobURL = blob => { const url = URL.createObjectURL(blob); urls.add(url); return url; };
let adapter;
let initialized = false;
let commandQueue = Promise.resolve();
let turning = false;
const turn = direction => {
  if (turning) return;
  turning = true;
  commandQueue = commandQueue.then(() => adapter?.turn(direction)).catch(error).finally(() => { turning = false; });
};
const removeInteractions = installReaderInteractions(document, {
  scrollContainer: root,
  send, turn, canSwipe: () => !!adapter && adapter.canSwipe(),
  canPage: () => !!adapter,
  scrolled: () => adapter?.scrolled?.() ?? false,
  scrollAtBoundary: direction => adapter?.scrollAtBoundary?.(direction) ?? false,
  atScrollBoundary: direction => adapter?.atScrollBoundary?.(direction) ?? false,
});
function applyFrameTheme() {
  document.body.style.backgroundColor = theme.background;
  document.body.style.color = theme.text;
  root.style.backgroundColor = theme.background;
}
applyFrameTheme();

window.addEventListener("message", event => {
  const m = event.data;
  if (event.source !== parent || !m || m.channel !== "grafium-book" || m.token !== token) return;
  if (adapter && m.type === "turn" && ["prev", "next", "left", "right"].includes(m.direction)) {
    turn(m.direction);
    return;
  }
  if (m.type === "shortcuts") {
    setReaderShortcutBindings(m.bindings);
    return;
  }
  commandQueue = commandQueue.then(async () => {
    if (m.type === "theme" && isReaderTheme(m.theme)) {
      theme = m.theme;
      applyFrameTheme();
      adapter?.theme();
    } else if (m.type === "open" && !initialized) {
      if (!(m.bytes instanceof ArrayBuffer) || !["epub", "fb2", "mobi", "azw3", "pdf"].includes(m.format))
        throw new Error("Invalid book transfer");
      initialized = true;
      adapter = m.format === "pdf" ? await openPDF(m) : await openReflowable(m);
    } else if (adapter) {
      if (m.type === "next") await adapter.next();
      else if (m.type === "prev") await adapter.prev();
      else if (m.type === "turn" && ["prev", "next", "left", "right"].includes(m.direction)) await adapter.turn(m.direction);
      else if (m.type === "flow" && ["paginated", "scrolled"].includes(m.value)) await adapter.flow?.(m.value);
      else if (m.type === "bionic" && typeof m.enabled === "boolean") await adapter.bionic?.(m.enabled);
      else if (m.type === "goto" && isBookLocation(m.location)) await adapter.goTo(m.location, m.select === true);
      else if (m.type === "capture-bookmark" && typeof m.requestId === "string" && /^[a-zA-Z0-9-]{1,80}$/.test(m.requestId)) {
        if (!adapter.captureBookmark) throw new Error("Private bookmarks are unavailable for this reader.");
        send("bookmark-captured", { requestId: m.requestId, ...adapter.captureBookmark() });
      }
      else if (m.type === "toc" && (typeof m.target === "string" || Number.isSafeInteger(m.target)))
        await adapter.toc(m.target);
      else if (m.type === "size" && Number.isFinite(m.value) && m.value >= 75 && m.value <= 200)
        await adapter.size(m.value);
      else if (m.type === "notes" && Array.isArray(m.locations) && m.locations.length <= 10000)
        await adapter.notes(m.locations.filter(isBookLocation));
      else if (m.type === "read-aloud-segments" && typeof m.requestId === "string"
        && /^[a-zA-Z0-9-]{1,80}$/.test(m.requestId)
        && Number.isSafeInteger(m.section) && m.section >= 0
        && Number.isSafeInteger(m.offset) && m.offset >= 0)
        await adapter.segments?.(m);
    }
  }).catch(error);
});
window.addEventListener("unload", () => {
  removeInteractions();
  adapter?.destroy();
  for (const url of urls) URL.revokeObjectURL(url);
});

function tocItems(items, depth = 0, output = []) {
  if (depth > 30) return output;
  for (const item of items ?? []) {
    if (output.length >= 10000) break;
    if (typeof item.href === "string" && localBookLink(item.href))
      output.push({ label: String(item.label ?? "Untitled").slice(0, 8000), target: item.href, depth });
    tocItems(item.subitems, depth + 1, output);
  }
  return output;
}

async function openReflowable({ bytes, format, location }) {
  let book;
  let zip;
  const file = new File([bytes], `book.${format}`);
  if (format === "epub") {
    configure({ useWebWorkers: false });
    zip = new ZipReader(new BlobReader(file));
    const entries = await zip.getEntries();
    if (entries.length > 100000 || entries.reduce((n, e) => n + e.uncompressedSize, 0) > 512 * 1024 * 1024)
      throw new Error("This book exceeds the reader's 512 MiB unpacked safety limit.");
    const map = new Map(entries.map(e => [e.filename, e]));
    if (map.size !== entries.length) throw new Error("This EPUB contains duplicate archive paths.");
    book = await new EPUB({
      loadText: name => map.get(name)?.getData(new TextWriter()),
      loadBlob: name => map.get(name)?.getData(new BlobWriter()),
      getSize: name => map.get(name)?.uncompressedSize ?? 0,
      sha1: value => sha1(new TextEncoder().encode(value)),
    }).init();
    book.transformTarget.addEventListener("load", event => {
      if (event.detail.isScript) event.detail.allow = false;
    });
  } else if (format === "fb2") book = await makeFB2(file);
  else book = await new MOBI({ unzlib: unzlibSync }).open(file);

  if (!book.sections?.length) throw new Error("This book has no readable sections.");
  // Every format goes through this gate AFTER Foliate rewrites local resources.
  for (const section of book.sections) {
    const load = section.load.bind(section);
    const unload = section.unload?.bind(section);
    let cached;
    section.load = async () => {
      if (cached) return cached;
      const url = await load();
      if (typeof url !== "string" || !url.startsWith("blob:"))
        throw new Error("The reader refused a non-local book section.");
      const response = await fetch(url);
      if (!response.ok) throw new Error("Could not load this book section.");
      cached = blobURL(new Blob([sanitizeBookDocument(await response.text())], { type: "text/html" }));
      return cached;
    };
    section.unload = () => {
      if (cached) { URL.revokeObjectURL(cached); urls.delete(cached); cached = null; }
      unload?.();
    };
  }
  // Audio overlays can drive navigation and are not part of this offline reader.
  for (const section of book.sections) section.mediaOverlay = null;
  const view = new View();
  installBionicCFI(view);
  root.append(view);
  let highlights = [];
  const fixed = book.rendition?.layout === "pre-paginated";
  let fontSize = 100;
  let bionic = false;
  const margin = 24;
  const interactions = new Map();
  const fitInlineSize = () => {
    if (fixed) return;
    const doc = view.renderer?.getContents()[0]?.doc;
    if (!doc) return;
    const vertical = /^(vertical|sideways)/.test(doc.defaultView.getComputedStyle(doc.body).writingMode);
    const available = vertical ? root.clientHeight : root.clientWidth;
    if (available <= 0) return;
    const value = `${available}px`;
    if (view.renderer.getAttribute("max-inline-size") !== value)
      view.renderer.setAttribute("max-inline-size", value);
  };
  const styleBook = () => {
    if (fixed) return;
    for (const { doc } of view.renderer.getContents()) applyBookTheme(doc, theme, fontSize);
    view.renderer.setStyles(bookThemeStyles(theme));
  };
  const atScrollBoundary = direction => {
    const renderer = view.renderer;
    return renderer.scrolled && (direction === "next"
      ? renderer.viewSize - renderer.end <= 2 : renderer.start <= 2);
  };
  const scrollAtBoundary = direction => {
    if (!atScrollBoundary(direction)) return false;
    turn(direction);
    return true;
  };
  const step = direction => {
    const distance = view.renderer.scrolled ? Math.max(1, view.renderer.size - margin * 2) * .9 : undefined;
    return direction === "next" ? view.next(distance) : view.prev(distance);
  };
  const reflow = async change => {
    const cfi = view.lastLocation?.cfi;
    const selections = view.renderer.getContents().flatMap(({ doc, index }) => {
      const selection = doc.getSelection();
      if (!selection?.rangeCount || selection.isCollapsed) return [];
      const range = selection.getRangeAt(0);
      return [{ doc, cfi: view.getCFI(index, range), backward: selection.anchorNode === range.endContainer
        && selection.anchorOffset === range.endOffset }];
    });
    change();
    view.renderer.render();
    if (cfi) await view.goTo(cfi);
    for (const { doc, cfi, backward } of selections) {
      const range = view.resolveCFI(cfi).anchor(doc);
      const selection = doc.getSelection();
      selection.setBaseAndExtent(
        backward ? range.endContainer : range.startContainer, backward ? range.endOffset : range.startOffset,
        backward ? range.startContainer : range.endContainer, backward ? range.startOffset : range.endOffset);
    }
    await paint();
  };
  const locator = cfi => ({ kind: "epub", cfi, rendererVersion: BOOK_RENDERER_VERSION });
  const paint = async () => {
    if (fixed) return;
    for (const value of highlights) await view.addAnnotation({ value });
  };
  view.addEventListener("draw-annotation", event => event.detail.draw(Overlayer.highlight, { color: "#f3bd35" }));
  view.addEventListener("create-overlay", () => { void paint().catch(error); });
  view.addEventListener("show-annotation", () => send("open-notes"));
  view.addEventListener("external-link", event => event.preventDefault());
  view.addEventListener("link", event => {
    if (typeof event.detail.href !== "string" || !localBookLink(event.detail.href)) event.preventDefault();
    else send("navigation");
  });
  view.addEventListener("relocate", event => {
    const { cfi, tocItem, fraction } = event.detail;
    if (cfi) send("location", { location: locator(cfi),
      fraction: Number.isFinite(fraction) ? Math.max(0, Math.min(1, fraction)) : undefined,
      label: `${tocItem?.label ?? ""}${Number.isFinite(fraction) ? ` · ${Math.round(fraction * 100)}%` : ""}` });
  });
  view.addEventListener("load", event => {
    const { doc, index } = event.detail;
    for (const [previous, cleanup] of interactions) {
      if (!fixed || !previous.defaultView?.frameElement?.isConnected) { cleanup(); interactions.delete(previous); }
    }
    interactions.set(doc, installReaderInteractions(doc, {
      send, turn, canSwipe: () => !view.renderer.scrolled,
      scrolled: () => view.renderer.scrolled, scrollAtBoundary, atScrollBoundary,
    }));
    if (fixed) return;
    applyBookTheme(doc, theme, fontSize);
    setBookBionic(doc, bionic);
    fitInlineSize();
    let timer;
    doc.addEventListener("selectionchange", () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        const selection = doc.defaultView?.getSelection();
        const quote = selection?.toString();
        if (selection?.rangeCount && quote?.trim()) {
          try { send("selection", { quote: quote.slice(0, 200000), location: locator(view.getCFI(index, selection.getRangeAt(0))) }); }
          catch (e) { error(e); }
        }
      }, 80);
    });
  });
  await view.open(book);
  if (!fixed) {
    view.renderer.setAttribute("max-column-count", "1");
    view.renderer.setAttribute("gap", "7%");
    view.renderer.setAttribute("margin", `${margin}px`);
    view.renderer.setStyles(bookThemeStyles(theme));
  }
  const goTo = async (value, select = false) => {
    if (value.kind !== "epub" || value.rendererVersion !== BOOK_RENDERER_VERSION)
      throw new Error("The saved passage uses a different reader version; it was not guessed.");
    if (!await view.goTo(value.cfi)) throw new Error("The saved passage could not be found.");
    if (select && !fixed) {
      const resolved = view.resolveCFI(value.cfi);
      const content = view.renderer.getContents().find(content => content.index === resolved.index);
      if (!content) throw new Error("The bookmarked section could not be selected.");
      await view.renderer.scrollToAnchor(resolved.anchor(content.doc), true);
    }
  };
  const toc = tocItems(book.toc);
  if (!toc.length) book.sections.forEach((_, index) => toc.push({ label: `Section ${index + 1}`, target: index, depth: 0 }));
  const metadataLanguage = Array.isArray(book.metadata?.language) ? book.metadata.language[0] : book.metadata?.language;
  const language = typeof metadataLanguage === "string" && metadataLanguage.length <= 63
    && /^[a-zA-Z0-9]+(?:-[a-zA-Z0-9]+)*$/.test(metadataLanguage) ? metadataLanguage : undefined;
  send("ready", { toc, annotations: !fixed, language, direction: book.dir === "rtl" ? "rtl" : "ltr", notice: fixed
    ? "Fixed-layout book: passage selection and highlighting are unavailable. Whole-book notes remain available."
    : "Local book · text selections can be saved as passage notes. Book scripts and external resources are blocked." });
  if (location && location.kind === "epub" && location.rendererVersion === BOOK_RENDERER_VERSION) await goTo(location);
  else {
    await view.init({ showTextStart: true });
    if (location) error(new Error("Saved position belongs to a different reader version. Opened at the beginning."));
  }
  const resize = new ResizeObserver(fitInlineSize);
  if (!fixed) resize.observe(root);
  fitInlineSize();
  return {
    next: () => step("next"), prev: () => step("prev"), goTo,
    turn: direction => step(direction === "left" ? (book.dir === "rtl" ? "next" : "prev")
      : direction === "right" ? (book.dir === "rtl" ? "prev" : "next") : direction),
    canSwipe: () => !view.renderer.scrolled,
    scrolled: () => view.renderer.scrolled,
    scrollAtBoundary,
    atScrollBoundary,
    captureBookmark: () => {
      if (!fixed) {
        for (const { doc, index } of view.renderer.getContents()) {
          const selection = doc.getSelection();
          if (!selection?.rangeCount || !selection.toString().trim()) continue;
          const range = selection.getRangeAt(0);
          if (doc.body.contains(range.commonAncestorContainer))
            return { location: locator(view.getCFI(index, range)), quote: Array.from(range.toString()).slice(0, 240).join("") };
        }
      }
      const current = view.lastLocation;
      if (!current?.cfi) throw new Error("Wait for a visible page before bookmarking.");
      return { location: locator(current.cfi), quote: Array.from(current.range?.toString() ?? "").slice(0, 240).join("") };
    },
    flow: async value => {
      if (!fixed && view.renderer.scrolled !== (value === "scrolled"))
        await reflow(() => view.renderer.setAttribute("flow", value));
    },
    bionic: async enabled => {
      if (fixed || enabled === bionic) return;
      bionic = enabled;
      await reflow(() => {
        for (const { doc } of view.renderer.getContents()) setBookBionic(doc, enabled);
      });
    },
    theme: styleBook,
    segments: async ({ requestId, section, offset }) => {
      if (format !== "epub" || fixed) throw new Error("Read aloud requires a reflowable EPUB.");
      if (book.sections.length > 10000 || section >= book.sections.length)
        throw new Error("Invalid read-aloud section.");
      const url = await book.sections[section].load();
      const response = await fetch(url);
      if (!response.ok) throw new Error("Could not read the local EPUB section.");
      const doc = new DOMParser().parseFromString(await response.text(), "text/html");
      const walker = doc.createTreeWalker(doc.body, NodeFilter.SHOW_TEXT);
      const segments = [];
      let skipped = 0;
      let total = 0;
      let more = false;
      let node;
      outer: while ((node = walker.nextNode())) {
        if (node.parentElement?.closest("script,style,noscript,[hidden],[aria-hidden=true]")) continue;
        // 320 UTF-16 units remain below the native offline engine's 1,500-byte input cap.
        for (let start = 0, end = 0; start < node.textContent.length; start = end) {
          end = Math.min(start + 320, node.textContent.length);
          const before = node.textContent.charCodeAt(end - 1);
          const after = node.textContent.charCodeAt(end);
          if (before >= 0xD800 && before <= 0xDBFF && after >= 0xDC00 && after <= 0xDFFF) end--;
          const text = node.textContent.slice(start, end);
          if (!text.trim()) continue;
          if (skipped++ < offset) continue;
          if (segments.length >= 128 || total + text.length > 131072) { more = true; break outer; }
          const range = doc.createRange();
          range.setStart(node, start); range.setEnd(node, start + text.length);
          segments.push({ text, locator: locator(view.getCFI(section, range)) });
          total += text.length;
        }
      }
      send("read-aloud-segments", { requestId, section, sectionCount: book.sections.length,
        nextOffset: more ? offset + segments.length : null, segments });
    },
    toc: async target => {
      if (typeof target === "string" && !localBookLink(target)) throw new Error("External navigation is blocked.");
      if (!await view.goTo(target)) throw new Error("The contents entry could not be opened.");
    },
    size: value => {
      fontSize = value;
      styleBook();
    },
    notes: async locations => {
      if (fixed) return;
      for (const value of highlights) await view.deleteAnnotation({ value });
      highlights = [...new Set(locations.filter(l => l.kind === "epub" && l.rendererVersion === BOOK_RENDERER_VERSION).map(l => l.cfi))];
      await paint();
    },
    destroy: () => {
      resize.disconnect();
      for (const cleanup of interactions.values()) cleanup();
      interactions.clear();
      view.close(); book.destroy?.(); void zip?.close();
    },
  };
}

const assetBytes = name => {
  const encoded = PDF_LOCAL_ASSETS[name];
  if (!encoded) throw new Error(`Bundled PDF resource missing: ${name}`);
  return Uint8Array.from(atob(encoded), c => c.charCodeAt(0));
};
class CMapFactory { async fetch({ name }) { return { cMapData: assetBytes(`${name}.bcmap`), compressionType: 1 }; } }
class FontFactory { async fetch({ filename }) { return assetBytes(filename); } }
class WasmFactory { async fetch({ filename }) { return assetBytes(filename); } }

async function openPDF({ bytes, location }) {
  // Blob worker navigation loses opaque-origin identity in Chromium/WebKit.
  // This bundled classic data worker has its own opaque origin and no URL imports.
  const workerURL = `data:text/javascript;base64,${btoa(PDF_WORKER_SOURCE)}`;
  const worker = new Worker(workerURL);
  worker.addEventListener("error", event => error(new Error(`The offline PDF worker could not start: ${event.message || "unsupported WebView worker origin"}`)));
  const pdfWorker = new pdfjs.PDFWorker({ port: worker });
  const loading = pdfjs.getDocument({
    data: new Uint8Array(bytes), worker: pdfWorker,
    isEvalSupported: false, enableXfa: false, useWorkerFetch: false,
    CMapReaderFactory: CMapFactory, StandardFontDataFactory: FontFactory, WasmFactory,
    cMapPacked: true, useSystemFonts: false, isOffscreenCanvasSupported: false,
  });
  loading.onPassword = () => { error(new Error("Password-protected PDFs are not supported.")); void loading.destroy(); };
  const pdf = await loading.promise;
  const style = document.createElement("style");
  style.textContent = `
    #reader{overflow:auto}.pdf-page{position:relative;margin:12px auto;background:white;--scale-round-x:1px;--scale-round-y:1px}
    .pdf-page canvas{display:block}.textLayer{position:absolute;inset:0;overflow:clip;line-height:1;text-size-adjust:none;transform-origin:0 0}
    .textLayer :is(span,br){color:transparent;position:absolute;white-space:pre;cursor:text;transform-origin:0% 0%}
    .textLayer{--min-font-size:1;--text-scale-factor:calc(var(--total-scale-factor)*var(--min-font-size));--min-font-size-inv:calc(1/var(--min-font-size))}
    .textLayer>:not(.markedContent),.textLayer .markedContent span:not(.markedContent){z-index:1;--font-height:0;font-size:calc(var(--text-scale-factor)*var(--font-height));--scale-x:1;--rotate:0deg;transform:rotate(var(--rotate)) scaleX(var(--scale-x)) scale(var(--min-font-size-inv))}
    .textLayer .markedContent{display:contents}.textLayer ::selection{background:#3880ff66}
    .textLayer[data-main-rotation="90"]{transform:rotate(90deg) translateY(-100%)}
    .textLayer[data-main-rotation="180"]{transform:rotate(180deg) translate(-100%,-100%)}
    .textLayer[data-main-rotation="270"]{transform:rotate(270deg) translateX(-100%)}
    .pdf-highlights{position:absolute;inset:0;pointer-events:none}.pdf-highlights i{position:absolute;background:#f3bd3566}`;
  document.head.append(style);
  let pageNumber = location?.kind === "pdf" ? Math.min(location.page, pdf.numPages) : 1;
  let zoom = 100;
  let current;
  let highlights = [];
  let renderTask;
  let textLayer;
  let renderedWidth = -1;
  let destroyed = false;
  const clamp = value => Math.max(0, Math.min(1, value));
  const paint = () => {
    if (!current) return;
    const { overlay, viewport, page } = current;
    overlay.replaceChildren();
    const [x0, y0, x1, y1] = page.view;
    for (const location of highlights.filter(l => l.kind === "pdf" && l.page === pageNumber)) {
      for (const r of location.rects ?? []) {
        const [a, b, c, d] = viewport.convertToViewportRectangle([
          x0 + r.x * (x1 - x0), y1 - r.y * (y1 - y0),
          x0 + (r.x + r.width) * (x1 - x0), y1 - (r.y + r.height) * (y1 - y0),
        ]);
        const mark = document.createElement("i");
        Object.assign(mark.style, { left: `${Math.min(a, c)}px`, top: `${Math.min(b, d)}px`,
          width: `${Math.abs(c - a)}px`, height: `${Math.abs(d - b)}px` });
        overlay.append(mark);
      }
    }
  };
  const render = async () => {
    if (destroyed) return;
    const page = await pdf.getPage(pageNumber);
    if (destroyed) return;
    const base = page.getViewport({ scale: 1 });
    const width = root.clientWidth;
    const scale = Math.max(0.2, Math.min(3, (width - 28) / base.width)) * zoom / 100;
    const viewport = page.getViewport({ scale });
    const container = document.createElement("div");
    container.className = "pdf-page";
    container.style.width = `${viewport.width}px`;
    container.style.height = `${viewport.height}px`;
    container.style.setProperty("--total-scale-factor", String(scale * viewport.userUnit));
    const canvas = document.createElement("canvas");
    const dpr = Math.min(devicePixelRatio || 1, 2);
    canvas.width = Math.ceil(viewport.width * dpr); canvas.height = Math.ceil(viewport.height * dpr);
    canvas.style.width = `${viewport.width}px`; canvas.style.height = `${viewport.height}px`;
    const layer = document.createElement("div"); layer.className = "textLayer";
    const overlay = document.createElement("div"); overlay.className = "pdf-highlights";
    current = null;
    container.append(canvas, layer, overlay); root.replaceChildren(container);
    renderTask = page.render({ canvas, canvasContext: canvas.getContext("2d"), viewport,
      transform: dpr === 1 ? undefined : [dpr, 0, 0, dpr, 0, 0], annotationMode: pdfjs.AnnotationMode.DISABLE });
    await renderTask.promise;
    const textContent = await page.getTextContent();
    textLayer = new pdfjs.TextLayer({ textContentSource: textContent, container: layer, viewport });
    await textLayer.render();
    if (destroyed) return;
    current = { page, viewport, container, layer, overlay };
    renderedWidth = width;
    paint();
    send("location", { location: { kind: "pdf", page: pageNumber }, label: `Page ${pageNumber} of ${pdf.numPages}` });
  };
  const select = () => {
    const selection = getSelection();
    if (!current || !selection?.rangeCount || !selection.toString().trim()
      || !current.layer.contains(selection.anchorNode) || !current.layer.contains(selection.focusNode)) return;
    const { page, viewport, container } = current;
    const bound = container.getBoundingClientRect();
    const [x0, y0, x1, y1] = page.view;
    const rects = [...selection.getRangeAt(0).getClientRects()].filter(r => r.width && r.height).map(r => {
      const a = viewport.convertToPdfPoint(r.left - bound.left, r.top - bound.top);
      const b = viewport.convertToPdfPoint(r.right - bound.left, r.bottom - bound.top);
      const x = clamp((Math.min(a[0], b[0]) - x0) / (x1 - x0));
      const y = clamp((y1 - Math.max(a[1], b[1])) / (y1 - y0));
      return { x, y, width: Math.min(1 - x, Math.abs(b[0] - a[0]) / (x1 - x0)),
        height: Math.min(1 - y, Math.abs(b[1] - a[1]) / (y1 - y0)) };
    });
    if (rects.length > 1000) {
      error(new Error("This PDF selection is too large to anchor. Select a shorter passage."));
      return;
    }
    if (rects.length) send("selection", { quote: selection.toString().slice(0, 200000), location: { kind: "pdf", page: pageNumber, rects } });
  };
  document.addEventListener("selectionchange", select);
  const toc = [];
  const outline = async (items, depth = 0) => {
    if (depth > 30) return;
    for (const item of items ?? []) {
      if (toc.length >= 10000) return;
      const dest = typeof item.dest === "string" ? await pdf.getDestination(item.dest) : item.dest;
      if (Array.isArray(dest) && dest.length) {
        const index = Number.isInteger(dest[0]) ? dest[0] : await pdf.getPageIndex(dest[0]);
        toc.push({ label: String(item.title ?? "Untitled").slice(0, 8000), target: index + 1, depth });
      }
      await outline(item.items, depth + 1);
    }
  };
  await outline(await pdf.getOutline());
  send("ready", { toc, pages: pdf.numPages, annotations: true,
    notice: "Local PDF · text selections use page rectangles. Scans without a text layer cannot be selected. PDF scripts, forms and actions are disabled." });
  await render();
  let resizeTimer;
  const resize = new ResizeObserver(() => {
    // Observers fire once on attachment and on height-only changes. Replacing
    // an unchanged text layer would discard the user's current selection.
    if (destroyed || root.clientWidth === renderedWidth) return;
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      commandQueue = commandQueue.then(() => {
        if (!destroyed && root.clientWidth !== renderedWidth) return render();
      }).catch(error);
    }, 150);
  });
  resize.observe(root);
  const goTo = async location => {
    if (location.kind !== "pdf" || location.page > pdf.numPages) throw new Error("PDF passage page is unavailable.");
    pageNumber = location.page; await render(); root.scrollTop = 0;
    if (location.rects?.length) {
      highlights = [...highlights, location]; paint();
      current.overlay.firstElementChild?.scrollIntoView({ block: "center" });
    }
  };
  const step = async direction => {
    const forward = direction === "next" || direction === "right";
    const remaining = forward ? root.scrollHeight - root.clientHeight - root.scrollTop : root.scrollTop;
    if (remaining > 2) {
      root.scrollBy({ top: root.clientHeight * .9 * (forward ? 1 : -1), behavior: "instant" });
    } else {
      const page = Math.max(1, Math.min(pdf.numPages, pageNumber + (forward ? 1 : -1)));
      if (page === pageNumber) return;
      await goTo({ kind: "pdf", page });
      if (!forward) root.scrollTop = root.scrollHeight;
    }
  };
  return {
    next: () => goTo({ kind: "pdf", page: Math.min(pdf.numPages, pageNumber + 1) }),
    prev: () => goTo({ kind: "pdf", page: Math.max(1, pageNumber - 1) }),
    turn: step,
    canSwipe: () => zoom <= 100,
    scrolled: () => true,
    atScrollBoundary: direction => direction === "next"
      ? root.scrollHeight - root.clientHeight - root.scrollTop <= 2 : root.scrollTop <= 2,
    scrollAtBoundary: direction => {
      const remaining = direction === "next" ? root.scrollHeight - root.clientHeight - root.scrollTop : root.scrollTop;
      if (remaining > 2) return false;
      turn(direction); return true;
    },
    theme: () => {},
    goTo, toc: page => goTo({ kind: "pdf", page }),
    size: async value => { zoom = value; await render(); },
    notes: locations => { highlights = locations; paint(); },
    destroy: () => {
      destroyed = true;
      clearTimeout(resizeTimer); resize.disconnect(); document.removeEventListener("selectionchange", select);
      renderTask?.cancel(); textLayer?.cancel(); void pdf.destroy(); pdfWorker.destroy(); worker.terminate();
    },
  };
}
