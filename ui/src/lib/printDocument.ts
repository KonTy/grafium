/**
 * Turns resolved blocks into the HTML Grafium actually prints.
 *
 * This is deliberately a pure string builder with no DOM reads, so the same
 * function serves the on-screen preview, the printer and the PDF, and can be
 * tested without a window. It is also the seam a future manuscript exporter
 * can reuse rather than re-deriving "blocks to a document" from scratch.
 */

import { getHeadingLevel } from "./blockLayout";
import { renderBlock } from "./markdown";
import type { PrintBlock } from "./printScope";

/** Deeper nesting stops indenting so long outlines keep a usable text column. */
const MAX_PRINT_INDENT = 8;

export interface PrintDocumentOptions {
  /** Shown as the printed document's heading. */
  title: string;
  /** Optional line under the title, such as the graph name or the date. */
  subtitle?: string;
  blocks: readonly PrintBlock[];
  /** Folder that relative image links resolve against, as on screen. */
  assetBaseDir?: string;
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/**
 * Give every image a real `src`.
 *
 * On screen, graph images are left as `data-src` and filled in by an
 * intersection observer once they scroll into view. A printout has no
 * scrolling and no observer, so without this every local image would come out
 * of the printer as a blank gap.
 */
export function eagerImages(html: string): string {
  return html.replace(/<img\b[^>]*>/g, (tag) =>
    tag
      .replace(/\sloading="lazy"/g, "")
      .replace(/\sfetchpriority="[^"]*"/g, "")
      .replace(/\sdata-src="/g, ' src="'),
  );
}

/** Describe media that cannot be printed, instead of leaving a blank space. */
export function describeMedia(html: string): string {
  const note = (kind: string, attrs: string): string => {
    const asset = /\sdata-asset="([^"]*)"/.exec(attrs)?.[1];
    const src = /\ssrc="([^"]*)"/.exec(attrs)?.[1];
    const label = asset || src || "";
    const suffix = label ? `: ${label}` : "";
    return `<p class="print-media-note">[${kind}${suffix}]</p>`;
  };
  return html
    .replace(/<audio\b([^>]*)>[\s\S]*?<\/audio>/g, (_m, attrs: string) => note("audio", attrs))
    .replace(/<video\b([^>]*)>[\s\S]*?<\/video>/g, (_m, attrs: string) => note("video", attrs))
    .replace(/<iframe\b([^>]*)>[\s\S]*?<\/iframe>/g, (_m, attrs: string) => note("embed", attrs));
}

/** The printable HTML for one block's Markdown. */
export function renderPrintBlockContent(content: string, assetBaseDir = ""): string {
  return describeMedia(eagerImages(renderBlock(content, assetBaseDir)));
}

/**
 * Build the body of the print document.
 *
 * Outline notes print with bullets so the structure survives on paper, while
 * a flat page such as an imported book prints as plain prose. The shape of
 * the content decides, rather than a setting nobody would find.
 */
export function buildPrintDocument(options: PrintDocumentOptions): string {
  const { title, subtitle, blocks, assetBaseDir = "" } = options;
  const printable = blocks.filter((entry) => entry.block.content.trim() !== "");
  const outlined = printable.some((entry) => entry.depth > 0);

  const parts: string[] = [];
  parts.push('<header class="print-header">');
  parts.push(`<h1 class="print-title">${escapeHtml(title)}</h1>`);
  if (subtitle) parts.push(`<p class="print-subtitle">${escapeHtml(subtitle)}</p>`);
  parts.push("</header>");

  if (printable.length === 0) {
    parts.push('<p class="print-empty">There is nothing to print here.</p>');
    return parts.join("");
  }

  parts.push('<div class="print-body">');
  for (const { block, depth } of printable) {
    const level = getHeadingLevel(block.content);
    const indent = Math.min(depth, MAX_PRINT_INDENT);
    const classes = ["print-block"];
    if (level > 0) classes.push("print-block-heading", `print-h${level}`);
    else if (outlined) classes.push("print-block-bulleted");
    parts.push(
      `<section class="${classes.join(" ")}" style="--print-depth: ${indent}">` +
        `<div class="print-block-content">${renderPrintBlockContent(block.content, assetBaseDir)}</div>` +
        "</section>",
    );
  }
  parts.push("</div>");
  return parts.join("");
}
