import { marked } from "marked";
import katex from "katex";

// Custom renderer for code blocks with line numbers
const renderer = new marked.Renderer();
renderer.code = function ({ text, lang }: { text: string; lang?: string }) {
  const lines = text.split("\n");
  const lineNumbersHtml = lines
    .map((_, i) => `<span class="line-number">${i + 1}</span>`)
    .join("");
  const codeHtml = lines
    .map((line) => `<span class="code-line">${escapeHtml(line)}</span>`)
    .join("");
  const langLabel = lang ? `<span class="code-lang">${escapeHtml(lang)}</span>` : "";
  return `<div class="code-block-wrapper">${langLabel}<div class="code-block-inner"><div class="line-numbers">${lineNumbersHtml}</div><pre class="code-block-pre"><code>${codeHtml}</code></pre></div></div>`;
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
export function renderBlock(content: string): string {
  const cached = getCached(content);
  if (cached !== undefined) return cached;

  let processed = renderMathOutsideCodeFences(content);

  // Transform [[page links]] to clickable links
  processed = processed.replace(
    /\[\[([^\]]+)\]\]/g,
    '<a class="page-link" data-page="$1">[[$1]]</a>'
  );

  // Transform #tags
  processed = processed.replace(
    /#([a-zA-Z0-9_-]+)/g,
    '<a class="tag" data-tag="$1">#$1</a>'
  );

  // Transform ((block refs))
  processed = processed.replace(
    /\(\(([^)]+)\)\)/g,
    '<span class="block-ref" data-ref="$1">(($1))</span>'
  );

  // Handle task markers
  processed = processed.replace(/^TODO /, '<span class="task-marker todo">TODO</span> ');
  processed = processed.replace(/^DOING /, '<span class="task-marker doing">DOING</span> ');
  processed = processed.replace(/^DONE /, '<span class="task-marker done">DONE</span> ');
  processed = processed.replace(/^LATER /, '<span class="task-marker later">LATER</span> ');
  processed = processed.replace(/^NOW /, '<span class="task-marker now">NOW</span> ');

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
