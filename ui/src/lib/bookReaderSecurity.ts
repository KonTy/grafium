import DOMPurify from "dompurify";
import { isBookLocation, type BookLocation } from "./bookLocations";
import { DEFAULT_READER_THEME, isReaderTheme, type ReaderTheme } from "./bookReaderTheme";

// No remote, graph, file, IPC, plugin, or app-origin URLs are permitted inside a book.
export const BOOK_CONTENT_CSP = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline' blob:; img-src blob: data:; font-src blob: data:; media-src 'none'; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'";
export const BOOK_FRAME_SANDBOX = "allow-scripts allow-same-origin";
export const localBookLink = (href: string) => /^(?:filepos:\d+|kindle:pos:fid:[0-9a-v]+:off:[0-9a-v]+)$/i.test(href)
  || !!href && !/^[\s/\\]|[\u0000-\u001f]|^[^/#?]*:/u.test(href);

/** Called on fully resource-rewritten markup, never on live app DOM. CSP is the resource boundary. */
export function sanitizeBookDocument(source: string): string {
  const parsed = new DOMParser().parseFromString(source, "text/html");
  const viewport = parsed.querySelector('meta[name="viewport"]')?.getAttribute("content");
  const fixedDimensions = viewport?.match(/^\s*width\s*=\s*(\d+(?:\.\d+)?)\s*,\s*height\s*=\s*(\d+(?:\.\d+)?)\s*$/i);
  const clean = DOMPurify.sanitize(source, {
    WHOLE_DOCUMENT: true, ADD_TAGS: ["link"],
    FORBID_TAGS: ["script", "iframe", "frame", "object", "embed", "form", "input", "button",
      "textarea", "select", "base", "meta", "foreignObject", "animate", "set", "audio", "video"],
    FORBID_ATTR: ["srcdoc", "srcset", "ping", "autofocus", "contenteditable", "target", "download"],
    ALLOWED_URI_REGEXP: /^(?:filepos:\d+$|kindle:pos:fid:[0-9a-v]+:off:[0-9a-v]+$|blob:|data:image\/(?:png|jpeg|gif|webp);|[^a-z]|[a-z+.\-]+(?:[^a-z+.\-:]|$))/i,
  });
  const doc = new DOMParser().parseFromString(clean, "text/html");
  for (const el of doc.querySelectorAll("*")) {
    for (const attr of [...el.attributes]) {
      const name = attr.localName.toLowerCase();
      if (name === "href") {
        const value = attr.value;
        if (el.localName === "a" ? !localBookLink(value) : !value.startsWith("blob:") && !value.startsWith("#"))
          el.removeAttributeNode(attr);
      } else if (name === "src" || name === "poster") {
        if (!/^(blob:|data:image\/(?:png|jpeg|gif|webp);)/i.test(attr.value)) el.removeAttributeNode(attr);
      }
    }
    if (el.localName === "link" && el.getAttribute("rel") !== "stylesheet") el.remove();
  }
  const csp = doc.createElement("meta");
  csp.httpEquiv = "Content-Security-Policy";
  csp.content = BOOK_CONTENT_CSP;
  doc.head.prepend(csp);
  if (fixedDimensions) {
    const viewport = doc.createElement("meta");
    viewport.name = "viewport";
    viewport.content = `width=${fixedDimensions[1]},height=${fixedDimensions[2]}`;
    doc.head.append(viewport);
  }
  return `<!doctype html>${doc.documentElement.outerHTML}`;
}

export interface BookTocItem { label: string; target: string | number; depth: number }
export interface ReaderTextSegment { text: string; locator: Extract<BookLocation, { kind: "epub" }> }
export interface ReaderBookmarkCapture { location: BookLocation; quote: string }
export type ReaderMessage =
  | { type: "ready"; toc: BookTocItem[]; annotations: boolean; notice: string; pages?: number; language?: string; direction?: "ltr" | "rtl" }
  | { type: "location"; location: BookLocation; label: string; fraction?: number }
  | { type: "selection"; quote: string; location: BookLocation }
  | ({ type: "bookmark-captured"; requestId: string } & ReaderBookmarkCapture)
  | { type: "clear-selection" }
  | { type: "error"; message: string }
  | { type: "read-aloud-segments"; requestId: string; section: number; sectionCount: number; nextOffset: number | null; segments: ReaderTextSegment[] }
  | { type: "help" }
  | { type: "navigation" }
  | { type: "bookmark" }
  | { type: "toggle-bionic" }
  | { type: "toggle-controls" }
  | { type: "exit-fullscreen" }
  | { type: "toggle-fullscreen" }
  | { type: "open-notes" };

/** An opaque frame has origin "null"; source identity plus a per-mount secret is mandatory. */
export function readReaderMessage(event: MessageEvent, source: Window | null, token: string): ReaderMessage | null {
  if (!source || event.source !== source || event.origin !== "null" || !event.data
    || event.data.channel !== "grafium-book" || event.data.token !== token) return null;
  const m = event.data;
  switch (m.type) {
    case "bookmark-captured":
      return typeof m.requestId === "string" && /^[a-zA-Z0-9-]{1,80}$/.test(m.requestId)
        && isBookLocation(m.location) && typeof m.quote === "string" && m.quote.length <= 512 ? m : null;
    case "read-aloud-segments":
      return typeof m.requestId === "string" && /^[a-zA-Z0-9-]{1,80}$/.test(m.requestId)
        && Number.isSafeInteger(m.section) && m.section >= 0
        && Number.isSafeInteger(m.sectionCount) && m.sectionCount > m.section && m.sectionCount <= 10000
        && (m.nextOffset === null || Number.isSafeInteger(m.nextOffset) && m.nextOffset >= 0)
        && Array.isArray(m.segments) && m.segments.length <= 128
        && m.segments.every((segment: ReaderTextSegment) => segment && typeof segment.text === "string"
          && segment.text.trim().length > 0 && segment.text.length <= 2048
          && isBookLocation(segment.locator) && segment.locator.kind === "epub")
        && m.segments.reduce((sum: number, segment: ReaderTextSegment) => sum + segment.text.length, 0) <= 131072 ? m : null;
    case "ready":
      return Array.isArray(m.toc) && m.toc.length <= 10000 && m.toc.every((t: BookTocItem) =>
        t && typeof t.label === "string" && t.label.length < 8192
        && ((typeof t.target === "string" && t.target.length < 16384) || Number.isSafeInteger(t.target))
        && Number.isInteger(t.depth) && t.depth >= 0 && t.depth < 100)
        && typeof m.annotations === "boolean" && typeof m.notice === "string"
        && (m.direction === undefined || m.direction === "ltr" || m.direction === "rtl")
        && (m.language === undefined || typeof m.language === "string" && m.language.length <= 63
          && /^[a-zA-Z0-9]+(?:-[a-zA-Z0-9]+)*$/.test(m.language))
        && (m.pages === undefined || Number.isSafeInteger(m.pages) && m.pages > 0) ? m : null;
    case "location": return isBookLocation(m.location) && typeof m.label === "string"
      && (m.fraction === undefined || typeof m.fraction === "number" && Number.isFinite(m.fraction)
        && m.fraction >= 0 && m.fraction <= 1) ? m : null;
    case "selection": return isBookLocation(m.location) && typeof m.quote === "string"
      && m.quote.trim().length > 0 && m.quote.length <= 200000 ? m : null;
    case "clear-selection": case "open-notes": case "help": case "navigation": return m;
    case "toggle-controls": case "exit-fullscreen": case "toggle-fullscreen": case "bookmark": case "toggle-bionic":
      return Object.keys(m).every(key => ["channel", "token", "type"].includes(key)) ? { type: m.type } : null;
    case "error": return typeof m.message === "string" && m.message.length < 20000 ? m : null;
    default: return null;
  }
}

export function readerFrameURL(token: string, theme: ReaderTheme = DEFAULT_READER_THEME): string {
  if (!/^[a-zA-Z0-9-]+$/.test(token)) throw new Error("Invalid reader bridge token");
  if (!isReaderTheme(theme)) throw new Error("Invalid reader theme");
  const palette = Object.fromEntries(Object.keys(DEFAULT_READER_THEME).map(key => [key, theme[key as keyof ReaderTheme]]));
  const csp = `default-src 'none'; script-src 'nonce-${token}' 'wasm-unsafe-eval'; worker-src data:; connect-src blob:; frame-src 'self' blob: about:; style-src 'unsafe-inline' blob:; img-src blob: data:; font-src blob: data:; base-uri 'none'; form-action 'none'; object-src 'none'`;
  // A data document has a unique opaque origin even with allow-same-origin. srcdoc would
  // inherit the privileged app origin; never substitute it here. Nested book srcdoc frames
  // inherit ONLY this opaque origin, allowing Foliate's DOM APIs without app access.
  // Only a tiny trusted bootstrap belongs in the URL (Chromium rejects huge data URLs).
  // The parent transfers the offline bundle once; neither book frames nor remote senders
  // can supply executable code because source, channel, and mount token must all match.
  const bootstrap = `const token=${JSON.stringify(token)};function boot(e){const m=e.data;if(e.source!==parent||!m||m.channel!=="grafium-book"||m.token!==token||m.type!=="bootstrap"||typeof m.runtime!=="string")return;removeEventListener("message",boot);globalThis.GRAFIUM_BOOK_TOKEN=token;globalThis.GRAFIUM_BOOK_THEME=${JSON.stringify(palette)};const script=document.createElement("script");script.nonce=token;script.textContent=m.runtime;document.body.append(script)}addEventListener("message",boot);`;
  return "data:text/html;charset=utf-8," + encodeURIComponent(`<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="${csp}"><style>html,body,#reader{margin:0;width:100%;height:100%;overflow:hidden}body{background:${theme.background};color:${theme.text}}foliate-view{display:block;width:100%;height:100%}</style></head><body><div id="reader"></div><script nonce="${token}">${bootstrap}</script></body></html>`);
}
