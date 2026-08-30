import { marked } from "marked";
import katex from "katex";
import { invoke } from "@tauri-apps/api/core";

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
function cleanAssetPath(href: string): string {
  const h = href.trim();
  const rel = h.replace(/^([./]*\/)+/, "").replace(/^\.\.?\//, "");
  return rel.replace(/^(\.\.?\/)+/, "");
}

// Rewrite a markdown asset reference to a URL the webview can load.
// Local, graph-relative paths (e.g. `../assets/anki/gre/word.mp3` or
// `assets/img/foo.png`) are served through the custom `grafium-asset` scheme,
// which resolves them against the active graph root in the Rust backend.
// Absolute URLs (http/https/data/blob and grafium-asset itself) pass through.
function resolveAssetUrl(href: string): string {
  const h = href.trim();
  if (/^(https?:|data:|blob:|grafium-asset:)/i.test(h)) return h;
  return `grafium-asset://localhost/${encodeURI(cleanAssetPath(h))}`;
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

function escapeHtml(str: string): string {
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

// Configure marked for block rendering
marked.use({
  breaks: true,
  gfm: true,
  renderer,
});

// Simple LRU cache to avoid re-parsing unchanged blocks. Sized to comfortably
// cover a long page plus its backlinks, so blocks scrolled out of the
// virtualised window and back in are not re-parsed on every pass.
const cache = new Map<string, string>();
const MAX_CACHE = 2048;

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
 * Apply an inline transform only to text OUTSIDE fenced code blocks and inline
 * code spans, so that syntax like `#tag`, `[[link]]` or `((ref))` written
 * inside backticks is preserved verbatim (and not turned into HTML that marked
 * then escapes and shows as literal text).
 */
function transformOutsideCode(markdown: string, fn: (segment: string) => string): string {
  const codeRe = /```[\s\S]*?```|`[^`\n]*`/g;
  let out = "";
  let last = 0;

  for (const match of markdown.matchAll(codeRe)) {
    const start = match.index ?? 0;
    const end = start + match[0].length;
    out += fn(markdown.slice(last, start));
    out += match[0];
    last = end;
  }

  out += fn(markdown.slice(last));
  return out;
}

/**
 * Render a block's markdown content to HTML.
 * Handles [[page links]], #tags, ((block refs)), checkboxes, etc.
 */
export function renderBlock(content: string): string {
  const cached = getCached(content);
  if (cached !== undefined) return cached;

  let processed = renderMathOutsideCodeFences(content);

  // Unescape outline-style backslash escapes before brackets (e.g. \] → ])
  // so that standard markdown links like [text](url) render correctly.
  processed = processed.replace(/\\([[\]])/g, "$1");

  // Transform [[page links]], #tags and ((block refs)) — but only outside code
  // spans/fences so `#tag`-style examples inside backticks stay verbatim.
  processed = transformOutsideCode(processed, (segment) => {
    return segment
      .replace(
        /\[\[([^\]]+)\]\]/g,
        '<a class="page-link" data-page="$1">$1</a>'
      )
      .replace(
        /#([a-zA-Z0-9_-]+)/g,
        '<a class="tag" data-tag="$1">#$1</a>'
      )
      .replace(
        /\(\(([^)]+)\)\)/g,
        '<span class="block-ref" data-ref="$1">(($1))</span>'
      );
  });

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

  // Use full marked.parse for complete markdown support
  let html = marked.parse(processed) as string;

  // Strip wrapping <p>...</p> for single-paragraph content to avoid extra spacing
  const trimmed = html.trim();
  if (trimmed.startsWith("<p>") && trimmed.endsWith("</p>") && trimmed.indexOf("<p>", 3) === -1) {
    html = trimmed.slice(3, -4);
  }

  setCache(content, html);
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
