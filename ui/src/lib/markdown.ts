import { marked } from "marked";

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

/**
 * Render a block's markdown content to HTML.
 * Handles [[page links]], #tags, ((block refs)), checkboxes, etc.
 */
export function renderBlock(content: string): string {
  const cached = getCached(content);
  if (cached !== undefined) return cached;

  // Transform [[page links]] to clickable links
  let processed = content.replace(
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
