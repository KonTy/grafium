import DOMPurify from "dompurify";
import { marked } from "marked";
import katex from "katex";
import { invoke } from "@tauri-apps/api/core";
import { CALLOUT_KINDS, CALLOUT_META, type CalloutKind } from "./callouts";
import { iconHtmlForName } from "./emojiIconPicker";
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
let imageRenderIndex = 0;
type ImageSizeHint = { width: number | null; height: number | null };
let imageSizeHints: ImageSizeHint[] = [];

const MARKDOWN_IMAGE_RE = /(!\[[^\]\n]*\]\(\s*)(<[^>\n]+>|(?:\\.|[^)\s\n])+)((?:\s+["'][^)\n]*["'])?\s*\))(\s*\{:[^}\n]*\})?/g;
const IMAGE_WIDTH_IN_ATTR_RE = /(?:^|[,\s{]):?width\s+([0-9]{1,5})(?=$|[,\s}])/;
const IMAGE_HEIGHT_IN_ATTR_RE = /(?:^|[,\s{]):?height\s+([0-9]{1,5})(?=$|[,\s}])/;
const IMAGE_PIPE_SIZE_RE = /^(.*)\|([0-9]{1,5})(?:x([0-9]{1,5}))?$/;
const VIDEO_EMBED_BLOCK_RE = /^\s*\{\{video\s+(\S+)(?:\s+([^}]+?))?\}\}\s*$/i;
const YOUTUBE_HOST_RE = /^(?:www\.|m\.)?(?:youtube\.com|youtube-nocookie\.com)$/i;
const YOUTU_BE_HOST_RE = /^(?:www\.)?youtu\.be$/i;
const MIN_IMAGE_SIZE = 40;
const MAX_IMAGE_SIZE = 3200;

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

function isHttpUrl(url: string): boolean {
  return /^https?:\/\//i.test(url.trim());
}

function extOf(url: string): string {
  const noQuery = url.split(/[?#]/)[0];
  const dot = noQuery.lastIndexOf(".");
  return dot >= 0 ? noQuery.slice(dot + 1).toLowerCase() : "";
}

function normalizeImageSize(size: unknown): number | null {
  const parsed = typeof size === "number" ? size : Number(size);
  if (!Number.isFinite(parsed) || parsed <= 0) return null;
  return Math.max(MIN_IMAGE_SIZE, Math.min(MAX_IMAGE_SIZE, Math.round(parsed)));
}

function emptyImageSizeHint(): ImageSizeHint {
  return { width: null, height: null };
}

function splitMarkdownImageTargetSize(target: string): { target: string; size: ImageSizeHint } {
  const angleWrapped = target.startsWith("<") && target.endsWith(">") && target.length >= 2;
  const body = angleWrapped ? target.slice(1, -1) : target;
  const match = body.match(IMAGE_PIPE_SIZE_RE);
  if (!match) return { target, size: emptyImageSizeHint() };
  const cleanBody = match[1];
  const width = normalizeImageSize(match[2]);
  const height = normalizeImageSize(match[3]);
  return {
    target: angleWrapped ? `<${cleanBody}>` : cleanBody,
    size: { width, height },
  };
}

function parseImageSizeAttributes(attrs: string | undefined): ImageSizeHint {
  return {
    width: normalizeImageSize(attrs?.match(IMAGE_WIDTH_IN_ATTR_RE)?.[1]),
    height: normalizeImageSize(attrs?.match(IMAGE_HEIGHT_IN_ATTR_RE)?.[1]),
  };
}

function imageSizeAttributes(size: ImageSizeHint): string {
  const parts: string[] = [];
  if (size.width) parts.push(`:width ${size.width}`);
  if (size.height) parts.push(`:height ${size.height}`);
  return parts.length > 0 ? `{${parts.join(", ")}}` : "";
}

function transformMarkdownOutsideCode(content: string, transform: (segment: string) => string): string {
  const codeRe = /```[\s\S]*?```|`[^`\n]*`/g;
  let out = "";
  let last = 0;
  for (const match of content.matchAll(codeRe)) {
    const start = match.index ?? 0;
    out += transform(content.slice(last, start));
    out += match[0];
    last = start + match[0].length;
  }
  out += transform(content.slice(last));
  return out;
}

type ActiveFence = {
  char: "`" | "~";
  length: number;
};

function normalizeIndentedFenceDelimiters(content: string): string {
  const lines = content.split("\n");
  let active: ActiveFence | null = null;

  return lines
    .map((line) => {
      const cr = line.endsWith("\r") ? "\r" : "";
      const body = cr ? line.slice(0, -1) : line;
      const match = /^([ \t]*)(`{3,}|~{3,})(.*)$/.exec(body);
      if (!match) return line;

      const marker = match[2];
      const rest = match[3];
      const char = marker[0] as "`" | "~";

      if (!active) {
        active = { char, length: marker.length };
        return `${marker}${rest}${cr}`;
      }

      if (char === active.char && marker.length >= active.length && rest.trim() === "") {
        active = null;
        return `${marker}${cr}`;
      }

      return line;
    })
    .join("\n");
}

function splitPipeTableRow(line: string): string[] | null {
  const trimmed = line.trim();
  if (!trimmed.includes("|")) return null;

  let body = trimmed;
  if (body.startsWith("|")) body = body.slice(1);
  if (body.endsWith("|")) body = body.slice(0, -1);

  const cells: string[] = [];
  let current = "";
  let escaped = false;
  for (const ch of body) {
    if (escaped) {
      current += ch;
      escaped = false;
      continue;
    }
    if (ch === "\\") {
      current += ch;
      escaped = true;
      continue;
    }
    if (ch === "|") {
      cells.push(current.trim());
      current = "";
      continue;
    }
    current += ch;
  }
  cells.push(current.trim());

  return cells.some((cell) => cell.length > 0) ? cells : null;
}

function isTableDelimiterCells(cells: readonly string[]): boolean {
  return cells.length > 0 && cells.every((cell) => /^:?-{3,}:?$/.test(cell));
}

function isLoosePipeTableRow(line: string): boolean {
  const trimmed = line.trim();
  if (!trimmed.startsWith("|") || !trimmed.endsWith("|")) return false;
  const cells = splitPipeTableRow(line);
  return !!cells && cells.length >= 2 && cells.some((cell) => cell.length > 0);
}

function fenceMarker(line: string): ActiveFence | null {
  const match = /^\s*(`{3,}|~{3,})/.exec(line);
  if (!match) return null;
  const marker = match[1];
  return { char: marker[0] as "`" | "~", length: marker.length };
}

function toggleFenceState(line: string, active: ActiveFence | null): ActiveFence | null {
  const marker = fenceMarker(line);
  if (!marker) return active;
  if (!active) return marker;
  return marker.char === active.char && marker.length >= active.length ? null : active;
}

function formatPipeTableDelimiterRow(originalLine: string, cells: readonly string[], columnCount: number): string {
  const indent = originalLine.match(/^\s*/)?.[0] ?? "";
  const normalized = cells.slice(0, columnCount);
  while (normalized.length < columnCount) normalized.push("---");
  return `${indent}| ${normalized.join(" | ")} |`;
}

function normalizeLooseMarkdownTables(content: string): string {
  const lines = content.split("\n");
  let activeFence: ActiveFence | null = null;

  for (let i = 0; i < lines.length; i += 1) {
    activeFence = toggleFenceState(lines[i], activeFence);
    if (activeFence || i + 1 >= lines.length) continue;

    const headerCells = splitPipeTableRow(lines[i]);
    const delimiterCells = splitPipeTableRow(lines[i + 1]);
    if (!headerCells || !delimiterCells || !isTableDelimiterCells(delimiterCells)) continue;
    if (headerCells.length === delimiterCells.length) continue;

    lines[i + 1] = formatPipeTableDelimiterRow(lines[i + 1], delimiterCells, headerCells.length);
  }

  activeFence = null;
  for (let i = 0; i < lines.length - 1; i += 1) {
    activeFence = toggleFenceState(lines[i], activeFence);
    if (activeFence) continue;

    const headerCells = splitPipeTableRow(lines[i]);
    const nextCells = splitPipeTableRow(lines[i + 1]);
    const previousCells = i > 0 ? splitPipeTableRow(lines[i - 1]) : null;
    if (
      !headerCells ||
      !nextCells ||
      isTableDelimiterCells(headerCells) ||
      isTableDelimiterCells(nextCells) ||
      (previousCells && (isLoosePipeTableRow(lines[i - 1]) || isTableDelimiterCells(previousCells))) ||
      !isLoosePipeTableRow(lines[i]) ||
      !isLoosePipeTableRow(lines[i + 1])
    ) {
      continue;
    }

    lines.splice(i + 1, 0, formatPipeTableDelimiterRow(lines[i], [], headerCells.length));
    i += 1;
  }

  return lines.join("\n");
}

function stripImageSizeAttributes(content: string): string {
  imageSizeHints = [];
  return transformMarkdownOutsideCode(content, (segment) =>
    segment.replace(MARKDOWN_IMAGE_RE, (
      match,
      prefix: string,
      target: string,
      suffix: string,
      attrs: string | undefined,
    ) => {
      const pipe = splitMarkdownImageTargetSize(target);
      const attr = parseImageSizeAttributes(attrs);
      imageSizeHints.push({
        width: pipe.size.width ?? attr.width,
        height: pipe.size.height ?? attr.height,
      });
      return `${prefix}${pipe.target}${suffix}`;
    })
  );
}

export function setMarkdownImageSize(
  content: string,
  imageIndex: number,
  size: { width?: number | null; height?: number | null }
): string {
  if (imageIndex < 0) return content;
  const nextSize: ImageSizeHint = {
    width: normalizeImageSize(size.width),
    height: normalizeImageSize(size.height),
  };
  if (!nextSize.width && !nextSize.height) return content;

  let currentIndex = 0;
  return transformMarkdownOutsideCode(content, (segment) =>
    segment.replace(MARKDOWN_IMAGE_RE, (
      match,
      prefix: string,
      target: string,
      suffix: string,
    ) => {
      if (currentIndex++ !== imageIndex) return match;
      return `${prefix}${splitMarkdownImageTargetSize(target).target}${suffix}${imageSizeAttributes(nextSize)}`;
    })
  );
}

export function setMarkdownImageWidth(content: string, imageIndex: number, width: number): string {
  return setMarkdownImageSize(content, imageIndex, { width });
}

export function clearMarkdownImageWidth(content: string, imageIndex: number): string {
  if (imageIndex < 0) return content;

  let currentIndex = 0;
  return transformMarkdownOutsideCode(content, (segment) =>
    segment.replace(MARKDOWN_IMAGE_RE, (
      match,
      prefix: string,
      target: string,
      suffix: string,
    ) => {
      if (currentIndex++ !== imageIndex) return match;
      return `${prefix}${splitMarkdownImageTargetSize(target).target}${suffix}`;
    })
  );
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
  const index = imageRenderIndex++;
  if (AUDIO_EXTS.has(ext)) {
    const rel = escapeHtml(cleanAssetPath(href));
    return `<audio class="fc-audio" controls preload="none"${titleAttr} data-asset="${rel}"></audio>`;
  }
  if (VIDEO_EXTS.has(ext)) {
    const rel = escapeHtml(cleanAssetPath(href));
    return `<video class="fc-video" controls preload="metadata"${titleAttr} data-asset="${rel}"></video>`;
  }
  const src = resolveAssetUrl(href);
  const srcAttr = src.startsWith("grafium-asset:")
    ? ` data-src="${escapeHtml(src)}"`
    : ` src="${escapeHtml(src)}"`;
  const size = imageSizeHints[index] ?? emptyImageSizeHint();
  const sizeAttrs = size.width
    ? ` style="width: ${size.width}px; height: auto;" data-image-width="${size.width}"${size.height ? ` data-image-height="${size.height}"` : ""}`
    : "";
  const markdownSrcAttr = ` data-markdown-src="${escapeHtml(href)}"`;
  return `<img class="fc-img" loading="lazy" decoding="async" fetchpriority="low" data-image-index="${index}"${markdownSrcAttr}${srcAttr} alt="${alt}"${titleAttr}${sizeAttrs}>`;
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

renderer.heading = function (
  this: any,
  { tokens, depth }: { tokens: unknown[]; depth: number }
): string {
  const text = this.parser.parseInline(tokens);
  const slug = markdownHeadingSlug(plainTextFromHtml(text));
  return `<h${depth} id="${escapeHtml(slug)}">${text}</h${depth}>`;
};

export function markdownHeadingSlug(text: string): string {
  let slug = "";
  let lastWasSeparator = false;
  for (const ch of text.trim().toLowerCase()) {
    if (/[\p{Letter}\p{Number}]/u.test(ch)) {
      slug += ch;
      lastWasSeparator = false;
    } else if (slug && !lastWasSeparator) {
      slug += "-";
      lastWasSeparator = true;
    }
  }
  slug = slug.replace(/-+$/g, "");
  return slug || "heading";
}

function plainTextFromHtml(html: string): string {
  return html
    .replace(/<[^>]*>/g, "")
    .replace(/&nbsp;/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'");
}

function escapeHtml(str: string): string {
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function escapeAttr(str: string): string {
  return escapeHtml(str).replace(/'/g, "&#39;");
}

function cleanVideoUrl(raw: string): string {
  const trimmed = raw.trim();
  if (/^(?:www\.youtube\.com|m\.youtube\.com|youtube\.com|youtu\.be)\//i.test(trimmed)) {
    return `https://${trimmed}`;
  }
  return trimmed;
}

function parseYouTubeVideoId(rawUrl: string): string | null {
  let url: URL;
  try {
    url = new URL(cleanVideoUrl(rawUrl));
  } catch {
    return null;
  }

  const host = url.hostname.toLowerCase();
  let id: string | null = null;
  if (YOUTU_BE_HOST_RE.test(host)) {
    id = url.pathname.split("/").filter(Boolean)[0] ?? null;
  } else if (YOUTUBE_HOST_RE.test(host)) {
    const parts = url.pathname.split("/").filter(Boolean);
    if (url.pathname === "/watch") {
      id = url.searchParams.get("v");
    } else if (["embed", "shorts", "live"].includes(parts[0] ?? "")) {
      id = parts[1] ?? null;
    }
  }

  return id && /^[A-Za-z0-9_-]{6,64}$/.test(id) ? id : null;
}

function renderVideoEmbedBlock(content: string): string | null {
  const match = content.match(VIDEO_EMBED_BLOCK_RE);
  if (!match) return null;

  const rawUrl = match[1];
  const title = (match[2]?.trim() || "Embedded video").replace(/^["']|["']$/g, "");
  const youtubeId = parseYouTubeVideoId(rawUrl);
  if (youtubeId) {
    const src = `https://www.youtube-nocookie.com/embed/${encodeURIComponent(youtubeId)}`;
    return (
      `<div class="grafium-video-embed youtube-video">` +
      `<iframe src="${src}" title="${escapeAttr(title)}" loading="lazy" ` +
      `allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture; web-share" ` +
      `referrerpolicy="strict-origin-when-cross-origin" allowfullscreen></iframe>` +
      `</div>`
    );
  }

  const url = cleanVideoUrl(rawUrl);
  if (VIDEO_EXTS.has(extOf(url))) {
    if (isHttpUrl(url)) {
      return `<video class="fc-video grafium-video-file" controls preload="metadata" src="${escapeAttr(url)}"></video>`;
    }
    return `<video class="fc-video grafium-video-file" controls preload="metadata" data-asset="${escapeAttr(cleanAssetPath(url))}"></video>`;
  }

  if (isHttpUrl(url)) {
    return `<a class="external-link" style="color:var(--accent-cyan)" href="${escapeAttr(url)}">${escapeHtml(url)}<span class="external-link-icon" aria-hidden="true">↗</span></a>`;
  }
  return escapeHtml(content);
}

function taskCheckbox(state: string, checked: boolean): string {
  const action = checked ? "" : ' data-task-action="done"';
  const title = checked ? `${state} task` : "Mark done";
  return (
    `<span class="task-checkbox ${checked ? "checked" : "unchecked"} ${state.toLowerCase()}"` +
    ` role="checkbox" aria-checked="${checked}" tabindex="0" data-task-state="${state}"${action}` +
    ` title="${title}"></span>`
  );
}

const TASK_BLOCK_START_RE =
  /^\s*(?:TODO|DOING|DONE|CANCELED|CANCELLED|LATER|NOW)\b|^\s*(?:[-*+]\s*)?\[[^\]\n]\]\s*/i;
const LOGSEQ_LOGBOOK_OPEN_RE = /^\s*:LOGBOOK:\s*$/i;
const LOGSEQ_LOGBOOK_CLOSE_RE = /^\s*:END:\s*$/i;
const CLOSED_LINE_RE = /^\s*CLOSED:\s*\[[^\]]+\]\s*$/i;
const TASK_MARKER_RE = /^(TODO|DOING|DONE|CANCELED|CANCELLED|LATER|NOW)$/i;
const MARKDOWN_CHECKBOX_LINE_RE =
  /(^|\n)([ \t]*)(?:[-*+]\s*)?\[([^\]\n])\]\s*(?:(TODO|DOING|DONE|CANCELED|CANCELLED|LATER|NOW)\b\s*)?/gi;

function hideRenderedTaskMetadata(content: string): string {
  if (!TASK_BLOCK_START_RE.test(content)) return content;

  const lines = content.split("\n");
  const visible: string[] = [];
  let inLogbook = false;

  for (const line of lines) {
    const trimmed = line.trim();
    if (LOGSEQ_LOGBOOK_OPEN_RE.test(trimmed)) {
      inLogbook = true;
      continue;
    }
    if (inLogbook) {
      if (LOGSEQ_LOGBOOK_CLOSE_RE.test(trimmed)) inLogbook = false;
      continue;
    }
    if (CLOSED_LINE_RE.test(trimmed)) continue;
    visible.push(line);
  }

  return visible.join("\n").trimEnd();
}

function normalizeTaskState(state: string): string {
  const upper = state.toUpperCase();
  return upper === "CANCELLED" ? "CANCELED" : upper;
}

function taskStateFromCheckbox(mark: string, explicitState: string | undefined): string {
  if (explicitState && TASK_MARKER_RE.test(explicitState)) return normalizeTaskState(explicitState);

  switch (mark) {
    case "x":
    case "X":
      return "DONE";
    case "-":
      return "CANCELED";
    case "/":
      return "DOING";
    case ">":
      return "LATER";
    case "!":
      return "NOW";
    default:
      return "TODO";
  }
}

function isCheckedTaskState(state: string): boolean {
  return state === "DONE" || state === "CANCELED";
}

function taskMarker(state: string): string {
  return `<span class="task-marker ${state.toLowerCase()}">${state}</span>`;
}

function renderMarkdownCheckboxTasks(content: string): string {
  return transformMarkdownOutsideCode(content, (segment) =>
    segment.replace(MARKDOWN_CHECKBOX_LINE_RE, (_, prefix: string, indent: string, mark: string, explicitState?: string) => {
      const state = taskStateFromCheckbox(mark, explicitState);
      return `${prefix}${indent}${taskCheckbox(state, isCheckedTaskState(state))}${taskMarker(state)} `;
    })
  );
}

// Canonicalize tag / page hierarchy separators the way the backend does
// (core/src/parser/links.rs `normalize_title`: `\` → `/`). The navigation
// target, the displayed text and the colour hash must all use the same
// canonical form, otherwise a tag like `#test\child` would hash/display one way
// but click through to (and could create) a different `test\child` page.
function normalizeHierarchy(name: string): string {
  const normalized = name
    .replace(/\\/g, "/")
    .split("/")
    .map((part) => part.trim())
    .filter(Boolean)
    .join("/");
  return normalized.replace(/^(\d{4})[-_](\d{2})[-_](\d{2})$/, "$1-$2-$3");
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

const priorityExtension = {
  name: "priority",
  level: "inline" as const,
  start(src: string) {
    const i = src.search(/\[#([ABC])\]/i);
    return i < 0 ? undefined : i;
  },
  tokenizer(src: string) {
    const m = /^\[#([ABC])\]/i.exec(src);
    if (!m) return undefined;
    return { type: "priority", raw: m[0], priority: m[1].toUpperCase() };
  },
  renderer(token: { priority: string }) {
    const priority = escapeHtml(token.priority);
    return `<span class="priority priority-${priority}" title="Priority ${priority}">Priority ${priority}</span>`;
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

const iconShortcodeExtension = {
  name: "iconShortcode",
  level: "inline" as const,
  start(src: string) {
    const i = src.indexOf(":icon-");
    return i < 0 ? undefined : i;
  },
  tokenizer(src: string) {
    const match = /^:icon-([a-z0-9-]+):/.exec(src);
    if (!match) return;
    const html = iconHtmlForName(match[1]);
    if (!html) return;
    return { type: "iconShortcode", raw: match[0], html };
  },
  renderer(token: { html: string }) {
    return token.html;
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

marked.use({ extensions: [pageLinkExtension, priorityExtension, tagExtension, iconShortcodeExtension, blockRefExtension] });

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

  return withDisplay.replace(/(?<!\\)\$([^\n$]+?)(?<!\\)\$/g, (match: string, expr: string) => {
    if (!shouldRenderInlineMath(expr)) return match;
    return katex.renderToString(expr.trim(), {
      throwOnError: false,
      displayMode: false,
    });
  });
}

function shouldRenderInlineMath(expr: string): boolean {
  const trimmed = expr.trim();
  if (!trimmed) return false;
  if (/^\d/.test(trimmed) && /[A-Za-z]/.test(trimmed)) return false;
  const proseWords = trimmed.match(/[A-Za-z]{2,}/g) ?? [];
  return proseWords.length < 3;
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
  const previousImageRenderIndex = imageRenderIndex;
  const previousImageSizeHints = imageSizeHints;
  assetBaseDir = dir;
  imageRenderIndex = 0;
  imageSizeHints = [];
  let html: string;
  try {
    // A whole-block admonition (`#+BEGIN_TIP` … `#+END_TIP`) renders as a
    // styled callout wrapping the (recursively rendered) body.
    const callout = renderCalloutBlock(content);
    const videoEmbed = callout === null ? renderVideoEmbedBlock(content) : null;
    html = callout !== null ? callout : videoEmbed !== null ? videoEmbed : renderMarkdownContent(content);
  } finally {
    assetBaseDir = previousBaseDir;
    imageRenderIndex = previousImageRenderIndex;
    imageSizeHints = previousImageSizeHints;
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
      "class", "style", "id", "href", "src", "alt", "title", "loading", "decoding", "fetchpriority",
      "colspan", "rowspan", "start", "type",
      // Data attributes the click delegation in ChatView reads.
      "data-page-link", "data-tag", "data-block-ref", "data-src", "data-loaded-once",
      "data-image-index", "data-image-width", "data-image-height", "data-markdown-src",
      // KaTeX/MathML presentation attributes.
      "xmlns", "display", "encoding", "mathvariant", "stretchy", "viewBox",
      "width", "height", "d", "x1", "x2", "y1", "y2", "fill", "stroke",
      "aria-hidden",
    ],
    // Only these URL schemes may appear in href/src. `data:` is excluded even
    // for images: a data URL is a script-delivery vector in enough contexts
    // that allowing it here buys nothing an http(s) image doesn't.
    ALLOWED_URI_REGEXP: /^(?:https?:|mailto:|grafium-asset:|#|\/)/i,
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
  let processed = normalizeIndentedFenceDelimiters(hideRenderedTaskMetadata(content));
  processed = normalizeLooseMarkdownTables(processed);
  processed = renderMathOutsideCodeFences(processed);
  processed = stripImageSizeAttributes(processed);

  // Unescape outline-style backslash escapes before brackets (e.g. \] → ])
  // so that standard markdown links like [text](url) render correctly.
  processed = processed.replace(/\\([[\]])/g, "$1");

  // NOTE: [[page links]], #tags, icon shortcodes and ((block refs)) are NOT transformed here.
  // They are registered as marked inline tokenizers (see pageLinkExtension /
  // tagExtension / iconShortcodeExtension / blockRefExtension above) so they only ever apply to real
  // text tokens — never to link destinations or any code form.

  // Handle task markers
  processed = renderMarkdownCheckboxTasks(processed);
  processed = processed.replace(/^TODO\b:?\s*/i, `${taskCheckbox("TODO", false)}${taskMarker("TODO")} `);
  processed = processed.replace(/^DOING\b:?\s*/i, `${taskCheckbox("DOING", false)}${taskMarker("DOING")} `);
  processed = processed.replace(/^DONE\b:?\s*/i, `${taskCheckbox("DONE", true)}${taskMarker("DONE")} `);
  processed = processed.replace(/^LATER\b:?\s*/i, `${taskCheckbox("LATER", false)}${taskMarker("LATER")} `);
  processed = processed.replace(/^NOW\b:?\s*/i, `${taskCheckbox("NOW", false)}${taskMarker("NOW")} `);
  processed = processed.replace(/^CANCELED\b:?\s*/i, `${taskCheckbox("CANCELED", true)}${taskMarker("CANCELED")} `);
  processed = processed.replace(/^CANCELLED\b:?\s*/i, `${taskCheckbox("CANCELED", true)}${taskMarker("CANCELED")} `);

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

let imageObserver: IntersectionObserver | null = null;

function rememberImageIntrinsicSize(img: HTMLImageElement) {
  if (img.naturalWidth <= 0 || img.naturalHeight <= 0) return;
  img.setAttribute("width", String(img.naturalWidth));
  img.setAttribute("height", String(img.naturalHeight));
}

function lazyImageObserver(): IntersectionObserver | null {
  if (typeof IntersectionObserver === "undefined") return null;
  imageObserver ??= new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        const img = entry.target as HTMLImageElement;
        const src = img.dataset.src;
        if (!src) {
          imageObserver?.unobserve(img);
          continue;
        }
        if (entry.isIntersecting) {
          if (img.src !== src) img.src = src;
          if (img.complete) rememberImageIntrinsicSize(img);
          img.dataset.loadedOnce = "1";
        } else if (img.dataset.loadedOnce === "1") {
          img.removeAttribute("src");
        }
      }
    },
    { rootMargin: "1400px 0px" }
  );
  return imageObserver;
}

function hydrateLazyImages(root: HTMLElement): () => void {
  const imgs = Array.from(root.querySelectorAll<HTMLImageElement>("img.fc-img[data-src]"));
  if (imgs.length === 0) return () => {};

  const observer = lazyImageObserver();
  const loadListeners: Array<[HTMLImageElement, () => void]> = [];
  for (const img of imgs) {
    const onLoad = () => rememberImageIntrinsicSize(img);
    img.addEventListener("load", onLoad);
    loadListeners.push([img, onLoad]);
  }

  if (!observer) {
    for (const img of imgs) {
      const src = img.dataset.src;
      if (src && img.src !== src) img.src = src;
      if (img.complete) rememberImageIntrinsicSize(img);
    }
    return () => {
      for (const [img, onLoad] of loadListeners) {
        img.removeEventListener("load", onLoad);
      }
    };
  }

  for (const img of imgs) observer.observe(img);
  return () => {
    for (const img of imgs) {
      observer.unobserve(img);
      img.removeAttribute("src");
    }
    for (const [img, onLoad] of loadListeners) {
      img.removeEventListener("load", onLoad);
    }
  };
}

/**
 * Hydrate rendered graph-local media after `{@html renderBlock(...)}` mounts.
 * Audio/video use in-memory `data:` URLs because WebKitGTK's GStreamer backend
 * can't fetch from our custom scheme. Images are scheme URLs, but they are
 * assigned lazily and unloaded offscreen so a long scanned book cannot keep
 * hundreds of large decoded page figures resident while the user reads.
 *
 * Call this after `{@html renderBlock(...)}` has been inserted (e.g. from a
 * Svelte `$effect` keyed to the rendered content).
 */
export function hydrateAssetMedia(root: HTMLElement | null | undefined): () => void {
  if (!root) return () => {};
  const cleanupImages = hydrateLazyImages(root);
  const els = root.querySelectorAll<HTMLMediaElement>("audio[data-asset], video[data-asset]");
  void (async () => {
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
  })();
  return cleanupImages;
}
