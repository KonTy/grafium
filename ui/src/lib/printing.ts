/**
 * Drives printing: work out what the user means by "print this", build the
 * document from stored blocks, hand it to the printer or a PDF, and tidy up.
 *
 * The print document is mounted into `#print-root`, which lives outside the
 * app shell and is only ever visible to the printer. WebKitGTK prints the
 * live page, so the document has to be in it; `print.css` hides the app and
 * reveals this container under `@media print`.
 */

import { invoke } from "@tauri-apps/api/core";
import { getPage, listBlocks, type Block } from "./api";
import { flushAllPageEditors } from "./editorPersistence";
import { assetBaseDirFor } from "./markdown";
import { buildPrintDocument } from "./printDocument";
import {
  chapterIdForBlock,
  flattenBlocks,
  listChapterHeadings,
  resolvePrintBlocks,
  type ChapterHeading,
  type PrintBlock,
  type PrintScopeKind,
} from "./printScope";

export type PrintColour = "colour" | "mono";
export type PrintAction = "printer" | "pdf";

export interface PrintRequest {
  pageId: string;
  scope: PrintScopeKind;
  chapterId: string | null;
  colour: PrintColour;
}

/** Everything the print dialog needs to describe the choices on offer. */
export interface PrintSource {
  pageId: string;
  pageTitle: string;
  blocks: Block[];
  assetBaseDir: string;
  chapters: ChapterHeading[];
  /** The chapter the cursor is in, so the picker opens where the user is. */
  suggestedChapterId: string | null;
  selectedIds: string[];
}

const PRINT_ROOT_ID = "print-root";

/** Ids of the blocks the user has selected, read from what is on screen.
 *
 * Only the identifiers come from the DOM. Their text is always loaded from
 * the store afterwards, because the rendered page is not a complete copy of
 * the user's writing.
 */
export function selectedBlockIds(root: ParentNode = document): string[] {
  return [...root.querySelectorAll<HTMLElement>(".block-item.selected[data-block-id]")]
    .map((element) => element.dataset.blockId ?? "")
    .filter((id) => id !== "");
}

/** The block being edited, used to guess which chapter the user means. */
export function focusedBlockId(root: ParentNode = document): string | null {
  const element = root.querySelector<HTMLElement>(".block-item.editing[data-block-id]");
  return element?.dataset.blockId ?? null;
}

/**
 * Collect what could be printed for a page.
 *
 * Unsaved edits are flushed first: someone who hits Print mid-sentence should
 * get the sentence they can see, not the version from before they typed it.
 */
export async function loadPrintSource(pageId: string): Promise<PrintSource> {
  await flushAllPageEditors();
  const [page, blocks] = await Promise.all([getPage({ id: pageId }), listBlocks(pageId)]);
  const flat = flattenBlocks(blocks);
  const focused = focusedBlockId();
  return {
    pageId,
    pageTitle: page?.title?.trim() || "Untitled",
    blocks,
    assetBaseDir: assetBaseDirFor(page?.file_path),
    chapters: listChapterHeadings(flat),
    suggestedChapterId: focused ? chapterIdForBlock(flat, focused) : null,
    selectedIds: selectedBlockIds(),
  };
}

function printedOn(): string {
  return new Intl.DateTimeFormat(undefined, { dateStyle: "long" }).format(new Date());
}

/** Title and subtitle for the printed document, matching the chosen scope. */
export function printHeadings(
  source: PrintSource,
  request: Pick<PrintRequest, "scope" | "chapterId">,
): { title: string; subtitle: string } {
  const stamp = `Printed ${printedOn()}`;
  if (request.scope === "chapter") {
    const chapter = source.chapters.find((entry) => entry.id === request.chapterId);
    if (chapter) return { title: chapter.text, subtitle: `${source.pageTitle} · ${stamp}` };
  }
  if (request.scope === "selection") {
    return { title: source.pageTitle, subtitle: `Selected blocks · ${stamp}` };
  }
  return { title: source.pageTitle, subtitle: stamp };
}

/** The document a request would produce, used for both preview and printing. */
export function buildRequestedDocument(source: PrintSource, request: PrintRequest): string {
  const { title, subtitle } = printHeadings(source, request);
  return buildPrintDocument({
    title,
    subtitle,
    blocks: requestedBlocks(source, request),
    assetBaseDir: source.assetBaseDir,
  });
}

/** The blocks a request resolves to, so callers can tell an empty one apart. */
export function requestedBlocks(source: PrintSource, request: PrintRequest): PrintBlock[] {
  return resolvePrintBlocks(source.blocks, {
    kind: request.scope,
    chapterId: request.chapterId,
    selectedIds: source.selectedIds,
  });
}

function printRoot(): HTMLElement {
  let root = document.getElementById(PRINT_ROOT_ID);
  if (!root) {
    root = document.createElement("div");
    root.id = PRINT_ROOT_ID;
    document.body.appendChild(root);
  }
  return root;
}

/** Put the document where the printer can see it. */
export function mountPrintDocument(html: string, colour: PrintColour): void {
  const root = printRoot();
  root.className = colour === "mono" ? "print-mono" : "";
  root.innerHTML = html;
}

/** Take it away again, so nothing is left behind for the next print. */
export function unmountPrintDocument(): void {
  const root = document.getElementById(PRINT_ROOT_ID);
  if (root) {
    root.innerHTML = "";
    root.className = "";
  }
}

export type PrintOutcome = "printed" | "cancelled" | "unsupported";

let jobInFlight = false;

interface AndroidPrintBridge {
  print: (jobName: string) => void;
  isAvailable: () => boolean;
}

function androidPrintBridge(): AndroidPrintBridge | undefined {
  return (window as typeof window & { GrafiumPrintBridge?: AndroidPrintBridge })
    .GrafiumPrintBridge;
}

/** Whether this build can print at all, so the dialog can say so up front. */
export function printingAvailable(): boolean {
  const bridge = androidPrintBridge();
  if (bridge) {
    try {
      return bridge.isAvailable();
    } catch {
      return false;
    }
  }
  return true;
}

/**
 * Whether the system's own print dialog already offers saving to PDF.
 *
 * Android's does, so a separate "Save as PDF" button there would be a second
 * route to the same place. The desktop needs its own, because the GTK dialog
 * hides printing to a file behind a destination the user has to know to pick.
 */
export function systemDialogSavesPdf(): boolean {
  return androidPrintBridge() !== undefined;
}

/**
 * Print through Android's print framework.
 *
 * Android's WebView does not implement `window.print()`, so the platform
 * print service has to be reached from Kotlin. Its dialog includes "Save as
 * PDF" among the destinations, which covers both actions the print dialog
 * offers. Handing the job to that dialog is the last this code hears of it,
 * so a started job is reported as printed.
 */
function printViaAndroid(bridge: AndroidPrintBridge, jobName: string): Promise<PrintOutcome> {
  return new Promise((resolve, reject) => {
    const target = window as typeof window & {
      __GRAFIUM_PRINT_RESOLVE?: (started: boolean, message: string) => void;
    };
    const finish = (outcome: PrintOutcome | null, message: string) => {
      delete target.__GRAFIUM_PRINT_RESOLVE;
      clearTimeout(timer);
      if (outcome) resolve(outcome);
      else reject(new Error(message || "Could not start printing"));
    };
    // Android never reports back if the activity is torn down mid-handover.
    const timer = setTimeout(() => finish(null, "The printer did not respond"), 30000);
    target.__GRAFIUM_PRINT_RESOLVE = (started, message) =>
      finish(started ? "printed" : null, message);
    try {
      bridge.print(jobName);
    } catch (cause) {
      finish(null, String(cause));
    }
  });
}

/**
 * Wait for images to finish decoding.
 *
 * The print job renders the page as it stands, so an image that is still
 * loading when printing starts comes out as a blank gap. The timeout means a
 * single unreachable image delays the printout rather than blocking it.
 */
async function imagesReady(root: HTMLElement, timeoutMs = 5000): Promise<void> {
  const pending = [...root.querySelectorAll("img")]
    .filter((img) => !img.complete)
    .map(
      (img) =>
        new Promise<void>((resolve) => {
          img.addEventListener("load", () => resolve(), { once: true });
          img.addEventListener("error", () => resolve(), { once: true });
        }),
    );
  if (pending.length === 0) return;
  await Promise.race([
    Promise.all(pending),
    new Promise<void>((resolve) => setTimeout(resolve, timeoutMs)),
  ]);
}

/**
 * Print the mounted document.
 *
 * `window.print()` is unreliable under WebKitGTK, which is Grafium's main
 * desktop target, so the native operation is used wherever it exists and the
 * browser call is only a fallback for platforms without one.
 */
export async function sendToPrinter(
  action: PrintAction,
  pdfPath?: string,
  jobName = "Grafium",
): Promise<PrintOutcome> {
  // One job at a time: both share `#print-root`, so a second job starting
  // while the first is still rendering would print the wrong document.
  if (jobInFlight) throw new Error("A print job is already running");
  jobInFlight = true;
  try {
    await imagesReady(printRoot());
    const bridge = androidPrintBridge();
    if (bridge) return await printViaAndroid(bridge, jobName);
    const target =
      action === "pdf" && pdfPath ? { kind: "pdf", path: pdfPath } : { kind: "dialog" };
    const outcome = await invoke<PrintOutcome>("print_document", { target });
    if (outcome === "unsupported") {
      window.print();
      return "printed";
    }
    return outcome;
  } finally {
    jobInFlight = false;
  }
}
