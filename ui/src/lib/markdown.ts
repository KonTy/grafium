import DOMPurify from "dompurify";
import { marked } from "marked";
import katex from "katex";
import { invoke } from "@tauri-apps/api/core";
import { CALLOUT_KINDS, CALLOUT_META, type CalloutKind } from "./callouts";
import { tagColorVar } from "./tagColor";

// Custom renderer for code blocks with line numbers.
// Each line number is emitted as a CSS counter (::before) on its own
// .code-line, so the number always shares the same line box as its code and
// can never drift out of alignment (regardless of theme borders/fonts).
const renderer = new marked.Renderer();
renderer.code = function ({ text, lang }: { text: string; lang?: string }) {
  const lines = text.split("\n");
  const codeHtml = lines
    .map((line) => `<span class="code-line">${escapeHtml(line)}</span>`)
    .join("");
  const langLabel = lang ? `<span class="code-lang">${escapeHtml(lang)}</span>` : "";
  return `<div class="code-block-wrapper">${langLabel}<pre class="code-block-pre"><code>${codeHtml}</code></pre></div>`;
};

const AUDIO_EXTS = new Set(["mp3", "wav", "ogg", "oga", "opus", "m4a", "flac", "aac"]);
const VIDEO_EXTS = new Set(["mp4", "m4v", "webm", "mov", "mkv", "ogv"]);

// Normalize a markdown asset reference to a graph-relative path (no scheme, no
// leading ./ or ../). Used both for the custom scheme URL and for in-memory
// hydration of media elements.
/**
 * Directory the page being rendered *right now* lives in, graph-relative and
 * without a trailing slash (e.g. `pages/mybooks/coolbook`).
 *
 * `marked`'s renderer hooks are synchronous and give no place to pass context,
 * so this is module-level — but it is assigned only for the duration of a
 * single synchronous `renderBlock` call, never by callers. An earlier version
 * let components set it from a Svelte `$effect`, which is wrong in both
 * directions: effects run *after* the template has already rendered, and the
 * journal view mounts several pages at once, so one shared value cannot
 * describe all of them. The base directory is a render argument, not state.
 */
let assetBaseDir = "";

function normalizeBaseDir(dir: string): string {
  return dir.replace(/^\/+|\/+$/g, "");
}

/** Graph-relative directory of `filePath`, for use as a render base directory. */
export function assetBaseDirFor(filePath: string | null | undefined): string {
  if (!filePath) return "";
  const normalized = normalizeBaseDir(filePath.replace(/\\/g, "/"));
  const slash = normalized.lastIndexOf("/");
  return slash > 0 ? normalized.slice(0, slash) : "";
}

/**
 * Turn a markdown asset reference into a graph-root-relative path.
 *
 * Two shapes exist and they resolve differently on purpose:
 *
 *   - `../assets/x.png` — the historical form. It was always resolved against
 *     the graph root regardless of how deep the page sat, so it keeps doing
 *     exactly that; thousands of existing references depend on it.
 *   - `assets/x.png` or `./assets/x.png` — a plain relative path, resolved
 *     against the page's own directory, which is what Obsidian, VS Code and
 *     GitHub do. That is what lets media sit beside the page that uses it and
 *     survive the folder being copied somewhere else.
 *
 * A page-relative guess that turns out to be wrong is not fatal: the backend
 * falls back to the graph-root `assets/` folder when the page-local file does
 * not exist, so an older note that already used the plain form still renders.
 */
function cleanAssetPath(href: string): string {
  // Backslashes first: a reference written `..\assets\x.png` means the same
  // thing as `../assets/x.png`, and without normalizing, the historical
  // root-relative form went unrecognised and resolved against the page.
  const h = href.trim().replace(/\\/g, "/");
  // Only `../` (and a leading `/`) mean "from the graph root". A leading `./`
  // is an ordinary relative path and must resolve like the plain form does —
  // treating it as root-relative made `./assets/x.png` and `assets/x.png`,
  // which mean the same thing everywhere else, resolve to different files.
  const isLegacyRootRelative = /^\.\.\//.test(h) || h.startsWith("/");
  const rel = h.replace(/^\/+/, "").replace(/^(\.\.?\/)+/, "");

  if (isLegacyRootRelative || assetBaseDir === "") return rel;
  return `${assetBaseDir}/${rel}`;
}

// Rewrite a markdown asset reference to a URL the webview can load.
// Local, graph-relative paths (e.g. `../assets/anki/gre/word.mp3` or
// `assets/img/foo.png`) are served through the custom `grafium-asset` scheme,
// which resolves them against the active graph root in the Rust backend.
// Absolute URLs (http/https/data/blob and grafium-asset itself) pass through.
function resolveAssetUrl(href: string): string {
  const h = href.trim();
  if (/^(https?:|data:|blob:|grafium-asset:)/i.test(h)) return h;
  return `grafium-asset://localhost/${encodePathForUrl(cleanAssetPath(h))}`;
}

/**
 * Percent-encode a path for use as a URL path, one segment at a time.
 *
 * `encodeURI` is not enough: it leaves `#` and `?` alone because they are
 * legal URL *syntax*, so a page at `pages/C#/intro.md` produced a URL whose
 * path silently ended at `pages/C` and every image on it 404'd.
 */
function encodePathForUrl(path: string): string {
  return path.split("/").map(encodeURIComponent).join("/");
}

function extOf(url: string): string {
  const noQuery = url.split(/[?#]/)[0];
  const dot = noQuery.lastIndexOf(".");
  return dot >= 0 ? noQuery.slice(dot + 1).toLowerCase() : "";
}

// Media-aware image renderer: `![alt](path.ext)` becomes an <audio>/<video>/<img>
// element depending on the file extension. This powers audio and video
// flashcards (e.g. imported Anki pronunciation clips) as well as image cards.
//
// Audio/video use a `data-asset` attribute instead of a live `src`: WebKitGTK's
// GStreamer media backend cannot fetch from our custom `grafium-asset` scheme,
// so `hydrateAssetMedia()` loads their bytes as in-memory `data:` URLs after
// the HTML is mounted. Images load fine straight from the scheme.
renderer.image = function ({ href, title, text }: { href: string; title?: string | null; text?: string }) {
  if (!href) return escapeHtml(text ?? "");
  const ext = extOf(href);
  const alt = escapeHtml(text ?? "");
  const titleAttr = title ? ` title="${escapeHtml(title)}"` : "";
  if (AUDIO_EXTS.has(ext)) {
    const rel = escapeHtml(cleanAssetPath(href));
    return `<audio class="fc-audio" controls preload="none"${titleAttr} data-asset="${rel}"></audio>`;
  }
  if (VIDEO_EXTS.has(ext)) {
    const rel = escapeHtml(cleanAssetPath(href));
    return `<video class="fc-video" controls preload="metadata"${titleAttr} data-asset="${rel}"></video>`;
  }
  const src = resolveAssetUrl(href);
  return `<img class="fc-img" loading="lazy" src="${src}" alt="${alt}"${titleAttr}>`;
};

// Render links so external `http(s)` destinations are distinct from internal
// navigation *without relying on colour*: they leave the app, so they get the
// `.external-link` class, the accent-cyan token AND a persistent outbound-arrow
// marker (↗). The arrow is the affordance that survives colour-vision
// deficiency / greyscale, where cyan can converge with other link hues (guarded
// by themeContrast.test.ts). Internal `[[page]]` / `#tag` / `((ref))` anchors
// are separate inline tokens and never reach this renderer.
renderer.link = function (
  this: any,
  { href, title, tokens }: { href: string; title?: string | null; tokens: unknown[] }
): string {
  const text = this.parser.parseInline(tokens);
  const url = !href || UNSAFE_URL_SCHEME_RE.test(href) ? "#" : href;
  const titleAttr = title ? ` title="${escapeHtml(title)}"` : "";
  if (/^https?:\/\//i.test(url)) {
    return (
      `<a class="external-link" style="color:var(--accent-cyan)" href="${escapeHtml(url)}"${titleAttr}>` +
      `${text}<span class="external-link-icon" aria-hidden="true">↗</span></a>`
    );
  }
  return `<a href="${escapeHtml(url)}"${titleAttr}>${text}</a>`;
};

function escapeHtml(str: string): string {
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

// Canonicalize tag / page hierarchy separators the way the backend does
// (core/src/parser/links.rs `normalize_title`: `\` → `/`). The navigation
// target, the displayed text and the colour hash must all use the same
// canonical form, otherwise a tag like `#test\child` would hash/display one way
// but click through to (and could create) a different `test\child` page.
function normalizeHierarchy(name: string): string {
  return name.replace(/\\/g, "/");
}

// Configure marked for block rendering
marked.use({
  breaks: true,
  gfm: true,
  renderer,
});

// `[[page links]]`, `#tags` and `((block refs))` are implemented as proper
// marked *inline tokenizers* rather than a pre-parse regex over the raw source.
// This is what keeps them from corrupting valid Markdown: custom inline
// extensions run at each cursor position BEFORE the built-in tokenizers, so
//   * the built-in `link` tokenizer still consumes `[text](url)` wholesale —
//     a `#fragment` inside a link destination is never seen as a tag;
//   * fenced (``` and ~~~), indented and inline (single/multi-backtick) code is
//     tokenized by the block/codespan tokenizers and never inline-lexed, so
//     `#tag`-like syntax written inside any code form survives verbatim.
// Colour still flows entirely through the theme token system (no raw hex):
// `#tags` get a deterministic per-name hue via tagColor.ts; `((block refs))`
// use --accent-purple so they read as a distinct link type from `[[page links]]`
// (which keep --text-link).
const pageLinkExtension = {
  name: "pageLink",
  level: "inline" as const,
  start(src: string) {
    const i = src.indexOf("[[");
    return i < 0 ? undefined : i;
  },
  tokenizer(src: string) {
    const m = /^\[\[([^\]\n]+)\]\]/.exec(src);
    if (!m) return undefined;
    return { type: "pageLink", raw: m[0], name: m[1] };
  },
  renderer(token: { name: string }) {
    const target = escapeHtml(normalizeHierarchy(token.name));
    return `<a class="page-link" data-page="${target}">${target}</a>`;
  },
};

const tagExtension = {
  name: "tag",
  level: "inline" as const,
  start(src: string) {
    const i = src.search(/#[a-zA-Z0-9_/\\-]/);
    return i < 0 ? undefined : i;
  },
  tokenizer(src: string) {
    const m = /^#([a-zA-Z0-9_/\\-]+)/.exec(src);
    if (!m) return undefined;
    return { type: "tag", raw: m[0], name: m[1] };
  },
  renderer(token: { name: string }) {
    const norm = normalizeHierarchy(token.name);
    const safe = escapeHtml(norm);
    return `<a class="tag" data-tag="${safe}" style="color:${tagColorVar(norm)}">#${safe}</a>`;
  },
};

const blockRefExtension = {
  name: "blockRef",
  level: "inline" as const,
  start(src: string) {
    const i = src.indexOf("((");
    return i < 0 ? undefined : i;
  },
  tokenizer(src: string) {
    const m = /^\(\(([^)\n]+)\)\)/.exec(src);
    if (!m) return undefined;
    return { type: "blockRef", raw: m[0], ref: m[1] };
  },
  renderer(token: { ref: string }) {
    const ref = escapeHtml(token.ref);
    return `<span class="block-ref" data-ref="${ref}" style="color:var(--accent-purple)">((${ref}))</span>`;
  },
};

marked.use({ extensions: [pageLinkExtension, tagExtension, blockRefExtension] });

// Simple LRU cache to avoid re-parsing unchanged blocks
const cache = new Map<string, string>();
const MAX_CACHE = 512;

function getCached(key: string): string | undefined {
  const val = cache.get(key);
  if (val !== undefined) {
    // Move to end (most recently used)
    cache.delete(key);
    cache.set(key, val);
  }
  return val;
}

function setCache(key: string, val: string): void {
  if (cache.size >= MAX_CACHE) {
    // Delete oldest entry
    const first = cache.keys().next().value!;
    cache.delete(first);
  }
  cache.set(key, val);
}

function renderMathSegment(text: string): string {
  // Render display math first so $$...$$ is not consumed by inline matching.
  const withDisplay = text.replace(/(?<!\\)\$\$([\s\S]+?)(?<!\\)\$\$/g, (_, expr: string) => {
    return katex.renderToString(expr.trim(), {
      throwOnError: false,
      displayMode: true,
    });
  });

  return withDisplay.replace(/(?<!\\)\$([^\n$]+?)(?<!\\)\$/g, (_, expr: string) => {
    return katex.renderToString(expr.trim(), {
      throwOnError: false,
      displayMode: false,
    });
  });
}

function renderMathOutsideCodeFences(markdown: string): string {
  const fenceRe = /```[\s\S]*?```/g;
  let out = "";
  let last = 0;

  for (const match of markdown.matchAll(fenceRe)) {
    const start = match.index ?? 0;
    const end = start + match[0].length;
    out += renderMathSegment(markdown.slice(last, start));
    out += match[0];
    last = end;
  }

  out += renderMathSegment(markdown.slice(last));
  return out;
}

/**
 * Render a block's markdown content to HTML.
 * Handles [[page links]], #tags, ((block refs)), checkboxes, etc.
 */
export function renderBlock(content: string, baseDir = ""): string {
  // The base directory is part of the output — the same block renders
  // different asset URLs on different pages — so it has to be part of the key.
  // Keying on content alone meant navigating to another page served the
  // previous page's asset paths from cache.
  const dir = normalizeBaseDir(baseDir);
  const cacheKey = `${dir}\u0000${content}`;
  const cached = getCached(cacheKey);
  if (cached !== undefined) return cached;

  // Assigned for exactly this synchronous render so marked's renderer hooks,
  // which take no context argument, can see it. Restored afterwards so a
  // nested render (a callout body renders recursively) cannot leak its
  // directory to whatever called it.
  const previousBaseDir = assetBaseDir;
  assetBaseDir = dir;
  let html: string;
  try {
    // A whole-block admonition (`#+BEGIN_TIP` … `#+END_TIP`) renders as a
    // styled callout wrapping the (recursively rendered) body.
    const callout = renderCalloutBlock(content);
    html = callout !== null ? callout : renderMarkdownContent(content);
  } finally {
    assetBaseDir = previousBaseDir;
  }

  setCache(cacheKey, html);
  return html;
}

/** URL schemes that must never survive in a rendered link's `href`. */
const UNSAFE_URL_SCHEME_RE = /^\s*(?:javascript|data|vbscript):/i;

/**
 * Escape only tag-*start* `<` (one immediately followed by a letter, `!`, `?`
 * or `/`) so raw HTML can't form, while leaving a `<` used as a less-than /
 * math operator and markdown `>` blockquotes intact. Fenced and inline code
 * are skipped — `marked`'s renderers already escape their contents, so a
 * literal `<script>` written inside backticks stays safe and verbatim.
 */
function escapeRawHtmlOutsideCode(content: string): string {
  const codeRe = /```[\s\S]*?```|`[^`\n]*`/g;
  const escapeTags = (s: string) => s.replace(/<(?=[a-zA-Z!/?])/g, "&lt;");
  let out = "";
  let last = 0;
  for (const match of content.matchAll(codeRe)) {
    const start = match.index ?? 0;
    const end = start + match[0].length;
    out += escapeTags(content.slice(last, start));
    out += match[0];
    last = end;
  }
  out += escapeTags(content.slice(last));
  return out;
}

/** Neutralize dangerous URL schemes in rendered `<a href>` attributes. */
function stripUnsafeHrefs(html: string): string {
  return html.replace(
    /(<a\b[^>]*?\shref=")([^"]*)(")/gi,
    (whole, pre: string, url: string, post: string) =>
      UNSAFE_URL_SCHEME_RE.test(url) ? `${pre}#${post}` : whole
  );
}

/**
 * Render UNTRUSTED markdown (an LLM chat answer) to HTML safe for `{@html}`.
 *
 * Same pipeline as {@link renderBlock} — GFM, code fences with line numbers,
 * KaTeX, and the `[[page link]]` / `#tag` / `((block ref))` transforms — but
 * first neutralizes raw HTML in the source (so a model can't emit
 * `<script>` / `<img onerror=…>` / `<iframe>` etc.) and strips dangerous URL
 * schemes (`javascript:`, `data:`, `vbscript:`) from any markdown link.
 *
 * `marked` is deliberately NOT configured to sanitize (note content is the
 * user's own and may legitimately contain raw HTML), so anything a model
 * produced MUST go through this rather than `renderBlock` directly. KaTeX
 * output is safe here because `trust` defaults to false, disabling `\href`
 * and friends.
 */
export function renderAssistantMarkdown(content: string): string {
  const neutralized = escapeRawHtmlOutsideCode(content);
  return sanitizeAssistantHtml(stripUnsafeHrefs(renderBlock(neutralized)));
}

/**
 * Allowlist-sanitize rendered assistant HTML immediately before `{@html}`.
 *
 * The source-level pass above is a useful first line, but it cannot be the
 * only one: it decides what is "inside a code span" with a regex, while
 * `marked` decides with a real parser, and any disagreement between the two
 * turns text the escaper believed it had neutralized into live markup.
 * Mismatched backtick runs were one such disagreement. Escaping the *input*
 * also does nothing about markup this module itself builds — an image URL was
 * interpolated straight into a `src` attribute, so
 * `![x](https://h/x"onerror="alert(1))` closed the attribute and executed.
 *
 * Sanitizing the *output* removes that whole class: whatever the pipeline
 * produced, only known-safe elements and attributes survive. This matters more
 * than usual here because the content is a local model's output, which now
 * routinely quotes text fetched from arbitrary websites — so the untrusted
 * input is genuinely attacker-controlled, and the app's CSP is not set.
 */
function sanitizeAssistantHtml(html: string): string {
  // Rendering happens in the webview, where a DOM always exists. Guarding
  // keeps this importable from a plain Node context (tooling, SSR-style
  // tests) without silently shipping unsanitized HTML: with no DOM to parse
  // with, the safe answer is to escape everything rather than pass it through.
  if (typeof window === "undefined" || !window.document) {
    return escapeHtml(html);
  }
  return DOMPurify.sanitize(html, {
    ALLOWED_TAGS: [
      "p", "br", "hr", "div", "span",
      "strong", "em", "b", "i", "u", "s", "del", "ins", "mark", "small", "sub", "sup",
      "h1", "h2", "h3", "h4", "h5", "h6",
      "ul", "ol", "li",
      "blockquote", "pre", "code",
      "table", "thead", "tbody", "tfoot", "tr", "th", "td",
      "a", "img",
      // KaTeX renders to these; dropping them would break every formula.
      "math", "semantics", "annotation", "mrow", "mi", "mn", "mo", "ms", "mtext",
      "mspace", "msup", "msub", "msubsup", "mfrac", "msqrt", "mroot", "mstyle",
      "munder", "mover", "munderover", "mtable", "mtr", "mtd", "mpadded",
      "mphantom", "menclose", "mglyph", "svg", "path", "line",
    ],
    ALLOWED_ATTR: [
      "class", "style", "href", "src", "alt", "title", "loading",
      "colspan", "rowspan", "start", "type",
      // Data attributes the click delegation in ChatView reads.
      "data-page-link", "data-tag", "data-block-ref",
      // KaTeX/MathML presentation attributes.
      "xmlns", "display", "encoding", "mathvariant", "stretchy", "viewBox",
      "width", "height", "d", "x1", "x2", "y1", "y2", "fill", "stroke",
      "aria-hidden",
    ],
    // Only these URL schemes may appear in href/src. `data:` is excluded even
    // for images: a data URL is a script-delivery vector in enough contexts
    // that allowing it here buys nothing an http(s) image doesn't.
    ALLOWED_URI_REGEXP: /^(?:https?:|mailto:|#|\/)/i,
    // Belt and braces: no event handlers survive regardless of the allowlist.
    FORBID_ATTR: ["onerror", "onload", "onclick", "onmouseover", "onfocus", "onanimationend"],
    FORBID_TAGS: ["script", "style", "iframe", "object", "embed", "form", "input", "button"],
  });
}

const CALLOUT_BLOCK_RE = new RegExp(
  `^\\s*#\\+BEGIN_(${CALLOUT_KINDS.join("|")})\\s*\\n([\\s\\S]*?)\\n?#\\+END_(${CALLOUT_KINDS.join("|")})\\s*$`,
  "i"
);

function renderCalloutBlock(content: string): string | null {
  const match = content.match(CALLOUT_BLOCK_RE);
  if (!match) return null;
  const beginKind = match[1].toLowerCase();
  const endKind = match[3].toLowerCase();
  if (beginKind !== endKind) return null;
  const kind = beginKind as CalloutKind;
  const meta = CALLOUT_META[kind];
  const body = match[2];
  const bodyHtml = body.trim() ? renderMarkdownContent(body) : "";
  return (
    `<div class="callout callout-${kind}">` +
    `<div class="callout-title">${meta.icon} ${escapeHtml(meta.title)}</div>` +
    `<div class="callout-body">${bodyHtml}</div>` +
    `</div>`
  );
}

/** Render arbitrary markdown/outliner content (links, tags, math, tasks). */
function renderMarkdownContent(content: string): string {
  let processed = renderMathOutsideCodeFences(content);

  // Unescape outline-style backslash escapes before brackets (e.g. \] → ])
  // so that standard markdown links like [text](url) render correctly.
  processed = processed.replace(/\\([[\]])/g, "$1");

  // NOTE: [[page links]], #tags and ((block refs)) are NOT transformed here.
  // They are registered as marked inline tokenizers (see pageLinkExtension /
  // tagExtension / blockRefExtension above) so they only ever apply to real
  // text tokens — never to link destinations or any code form.

  // Handle task markers
  processed = processed.replace(/^TODO\s+/i, '<span class="task-marker todo">TODO</span> ');
  processed = processed.replace(/^DOING\s+/i, '<span class="task-marker doing">DOING</span> ');
  processed = processed.replace(/^DONE\s+/i, '<span class="task-marker done">DONE</span> ');
  processed = processed.replace(/^LATER\s+/i, '<span class="task-marker later">LATER</span> ');
  processed = processed.replace(/^NOW\s+/i, '<span class="task-marker now">NOW</span> ');
  processed = processed.replace(/^CANCELED\s+/i, '<span class="task-marker canceled">CANCELED</span> ');

  // Handle priority markers [#A], [#B], [#C]
  processed = processed.replace(
    /\[#([ABC])\]/g,
    '<span class="priority priority-$1">[#$1]</span>'
  );

  // Handle SCHEDULED and DEADLINE timestamps (display as badges)
  processed = processed.replace(
    /SCHEDULED:\s*<([^>]+)>/g,
    '<span class="task-date scheduled" title="Scheduled">📅 $1</span>'
  );
  processed = processed.replace(
    /DEADLINE:\s*<([^>]+)>/g,
    '<span class="task-date deadline" title="Deadline">⏰ $1</span>'
  );

  // Use full marked.parse for complete markdown support. Inline tokenizers
  // handle [[page]] / #tag / ((ref)); renderer.link handles external links.
  let html = marked.parse(processed) as string;

  // Strip wrapping <p>...</p> for single-paragraph content to avoid extra spacing
  const trimmed = html.trim();
  if (trimmed.startsWith("<p>") && trimmed.endsWith("</p>") && trimmed.indexOf("<p>", 3) === -1) {
    html = trimmed.slice(3, -4);
  }

  return html;
}

// In-memory cache of resolved data: URLs so re-rendering the same card (or
// flipping front/back) doesn't re-read the file each time.
const mediaUrlCache = new Map<string, string>();

/**
 * Load <audio>/<video> media inside a rendered container as in-memory `data:`
 * URLs. WebKitGTK's GStreamer media backend can't fetch from our custom
 * `grafium-asset` scheme, so audio/video are emitted with a `data-asset`
 * attribute (see the image renderer) and their real `src` is filled in here
 * after the HTML is mounted in the DOM.
 *
 * Call this after `{@html renderBlock(...)}` has been inserted (e.g. from a
 * Svelte `$effect` keyed to the rendered content).
 */
export async function hydrateAssetMedia(root: HTMLElement | null | undefined): Promise<void> {
  if (!root) return;
  const els = root.querySelectorAll<HTMLMediaElement>("audio[data-asset], video[data-asset]");
  for (const el of Array.from(els)) {
    const rel = el.getAttribute("data-asset");
    if (!rel) continue;
    el.removeAttribute("data-asset");
    try {
      let url = mediaUrlCache.get(rel);
      if (!url) {
        url = await invoke<string>("read_asset_data_url", { path: rel });
        mediaUrlCache.set(rel, url);
      }
      el.src = url;
    } catch (e) {
      console.error("Failed to load media asset", rel, e);
    }
  }
}
