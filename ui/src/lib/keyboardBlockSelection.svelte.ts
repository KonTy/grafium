import type { Block } from "./api";
import { formatBlocksAsOutlineMarkdown, formatBlocksAsPlainText, writeClipboardText, type ClipboardBlock } from "./blockClipboard";
import { deleteBlockSelection } from "./blockSelectionActions";
import { hasKeyboardOverlay } from "./mainPaneScroll";
import { showToast } from "./toast.svelte";

export const KEYBOARD_BLOCK_SELECTION = Symbol("keyboard-block-selection");
export type SelectionDirection = "up" | "down";
export interface BlockTextSelection { anchor: number; head: number }
interface Endpoint { pageId: string; blockId: string }
export interface SelectionPage {
  visibleIds(): string[];
  prepare(blockId: string, isCurrent: () => boolean): Promise<boolean>;
  restore(blockId: string, selection: BlockTextSelection, isCurrent: () => boolean): Promise<void>;
  focusEdge(blockId: string, x: number, edge: "top" | "bottom", isCurrent: () => boolean): Promise<void>;
  focusCursor(blockId: string, edge: "start" | "end", isCurrent: () => boolean): Promise<void>;
  reveal(blockId: string): Promise<void>;
  select(ids: Set<string>): void;
  snapshots(ids: Set<string>): Block[];
  clipboard(ids: Set<string>): ClipboardBlock[];
  reload(): Promise<void>;
  indent(direction: "in" | "out"): Promise<void>;
  focusHost(): void;
}
interface SelectionSession {
  origin: Endpoint;
  text: BlockTextSelection;
  x: number;
  direction: SelectionDirection;
  trail: Endpoint[];
  nativeTrailLength: number;
}

export function createKeyboardBlockSelection(
  scope: () => HTMLElement | null,
  adjacentPage: (pageId: string, direction: SelectionDirection, isCurrent: () => boolean) => Promise<string | null>
    = async () => null,
) {
  const pages = new Map<string, SelectionPage>();
  let session = $state<SelectionSession | null>(null);
  let busy = $state(false);
  let revision = 0;
  let pending: { direction: SelectionDirection; source: Element; revision: number } | null = null;
  let deletionFocus: { source: Element; toolbar: boolean } | null = null;
  let destroyed = false;
  let highlighted = new Map<string, Set<string>>();

  function groups(trail = session?.trail ?? []) {
    const ordered = session?.direction === "up" ? [...trail].reverse() : trail;
    const result = new Map<string, Set<string>>();
    for (const point of ordered) {
      if (!result.has(point.pageId)) result.set(point.pageId, new Set());
      result.get(point.pageId)!.add(point.blockId);
    }
    return result;
  }

  function paint() {
    const next = groups();
    for (const id of new Set([...highlighted.keys(), ...next.keys()])) {
      pages.get(id)?.select(next.get(id) ?? new Set());
    }
    highlighted = next;
  }

  function clear() {
    revision += 1;
    pending = null;
    session = null;
    paint();
  }

  async function adjacent(point: Endpoint, direction: SelectionDirection, isCurrent: () => boolean): Promise<Endpoint | null> {
    const ids = pages.get(point.pageId)?.visibleIds() ?? [];
    const index = ids.indexOf(point.blockId);
    if (index < 0) throw new Error("The selected block is no longer available");
    const blockId = ids[index + (direction === "down" ? 1 : -1)];
    if (blockId) return { pageId: point.pageId, blockId };
    const pageId = await adjacentPage(point.pageId, direction, isCurrent);
    if (!pageId || !isCurrent()) return null;
    const nextIds = pages.get(pageId)?.visibleIds();
    const nextId = direction === "down" ? nextIds?.[0] : nextIds?.at(-1);
    if (!nextId) throw new Error("The adjacent entry has no editable block");
    return { pageId, blockId: nextId };
  }

  function report(error: unknown) {
    console.error("Keyboard block selection failed:", error);
    showToast(error instanceof Error ? error.message : String(error), "error");
  }

  async function begin(pageId: string, blockId: string, direction: SelectionDirection, text: BlockTextSelection, x: number, headBlockId = blockId) {
    if (pending || busy || session) return;
    const source = document.activeElement;
    if (!source || !scope()?.contains(source)) return;
    const request = { direction, source, revision: ++revision };
    pending = request;
    const isCurrent = () => !destroyed && pending === request && revision === request.revision
      && document.activeElement === source && source.isConnected;
    try {
      const origin = { pageId, blockId };
      const target = await adjacent({ pageId, blockId: headBlockId }, direction, isCurrent);
      if (!target || !isCurrent()) return;
      const page = pages.get(pageId);
      if (!page) throw new Error("The current editor is no longer available");
      if (target.pageId !== pageId && !await pages.get(target.pageId)?.prepare(target.blockId, isCurrent)) return;
      if (!await page.prepare(blockId, isCurrent) || pending !== request || destroyed) return;
      const ids = page.visibleIds();
      const start = ids.indexOf(blockId);
      const end = ids.indexOf(headBlockId);
      if (start < 0 || end < 0) throw new Error("The selection changed while saving");
      const initial = ids.slice(Math.min(start, end), Math.max(start, end) + 1);
      if (direction === "up") initial.reverse();
      session = { origin, text, x, direction, nativeTrailLength: initial.length,
        trail: [...initial.map((id) => ({ pageId, blockId: id })), target] };
      paint();
      page.focusHost();
      await pages.get(target.pageId)?.reveal(target.blockId);
    } catch (error) {
      if (pending === request) report(error);
    } finally {
      if (pending === request) pending = null;
    }
  }

  async function restore(collapse = false) {
    const previous = session;
    if (!previous) return;
    clear();
    const token = revision;
    const text = collapse ? { anchor: previous.text.anchor, head: previous.text.anchor } : previous.text;
    await pages.get(previous.origin.pageId)?.restore(previous.origin.blockId, text,
      () => !destroyed && revision === token && !!scope()?.contains(document.activeElement));
  }

  async function extend(direction: SelectionDirection) {
    const current = session;
    if (!current || pending || busy) return;
    if (direction !== current.direction) {
      revision += 1;
      if (current.trail.length === current.nativeTrailLength + 1) {
        await restore();
        return;
      }
      current.trail = current.trail.slice(0, -1);
      paint();
      const head = current.trail.at(-1)!;
      await pages.get(head.pageId)?.reveal(head.blockId);
      return;
    }
    const source = document.activeElement;
    if (!source) return;
    const request = { source, direction, revision: ++revision };
    pending = request;
    const isCurrent = () => !destroyed && session === current && pending === request
      && revision === request.revision && document.activeElement === source;
    try {
      const next = await adjacent(current.trail.at(-1)!, direction, isCurrent);
      if (!next || !isCurrent()) return;
      if (next.pageId !== current.trail.at(-1)!.pageId
        && !await pages.get(next.pageId)?.prepare(next.blockId, isCurrent)) return;
      if (!isCurrent()) return;
      current.trail = [...current.trail, next];
      paint();
      await pages.get(next.pageId)?.reveal(next.blockId);
    } finally {
      if (pending === request) pending = null;
    }
  }

  function payload() {
    const markdown: string[] = [];
    const plainText: string[] = [];
    for (const [pageId, ids] of groups()) {
      const page = pages.get(pageId);
      if (!page) throw new Error("A selected entry is no longer available");
      const blocks = page.clipboard(ids);
      markdown.push(formatBlocksAsOutlineMarkdown(blocks));
      plainText.push(formatBlocksAsPlainText(blocks));
    }
    return { markdown: markdown.join("\n"), plainText: plainText.join("\n") };
  }

  async function copy() {
    await writeClipboardText(payload().markdown);
    showToast("Selected blocks copied", "success");
  }

  async function remove() {
    if (!session || busy || pending) return;
    const selected = [...groups()].map(([pageId, ids]) => {
      const page = pages.get(pageId);
      if (!page) throw new Error("A selected entry is no longer available");
      return { pageId, blocks: page.snapshots(ids) };
    });
    const plans = selected.map(({ pageId, blocks }) => {
      const page = pages.get(pageId)!;
      const ids = page.visibleIds();
      const deleted = new Set(blocks.map((block) => block.id));
      const first = ids.findIndex((id) => deleted.has(id));
      if (first < 0) throw new Error("The selected blocks are no longer visible");
      return {
        pageId, page,
        after: ids.slice(first).filter((id) => !deleted.has(id)),
        before: ids.slice(0, first).filter((id) => !deleted.has(id)).reverse(),
        emptied: ids.every((id) => deleted.has(id)),
      };
    });
    const source = document.activeElement;
    if (!source) throw new Error("The selection has lost keyboard focus");
    const request = { source, toolbar: !!source.closest(".keyboard-selection-toolbar") };
    deletionFocus = request;
    const isCurrent = () => !destroyed && deletionFocus === request && !hasKeyboardOverlay(document)
      && (document.activeElement === source
        || (document.activeElement === document.body && (request.toolbar || !source.isConnected)))
      && plans.every(({ pageId, page }) => pages.get(pageId) === page);
    busy = true;
    try {
      try {
        await deleteBlockSelection(selected);
        clear();
      } finally {
        for (const { pageId, page } of plans) {
          if (pages.get(pageId) === page) await page.reload();
        }
      }
      if (!isCurrent()) return;
      // Collapse to the deletion gap in document order, independent of which
      // direction the selection was extended. Empty pages retain an editable row.
      for (const plan of plans) {
        const ids = plan.page.visibleIds();
        const surviving = new Set(ids);
        const next = plan.emptied ? ids[0] : plan.after.find((id) => surviving.has(id));
        if (next) {
          await plan.page.focusCursor(next, "start", isCurrent);
          return;
        }
      }
      for (const plan of [...plans].reverse()) {
        const surviving = new Set(plan.page.visibleIds());
        const previous = plan.before.find((id) => surviving.has(id));
        if (previous) {
          await plan.page.focusCursor(previous, "end", isCurrent);
          return;
        }
      }
      throw new Error("Could not locate an editable block after deleting the selection");
    } finally {
      busy = false;
      if (deletionFocus === request) deletionFocus = null;
    }
  }

  async function collapseToEdge(direction: SelectionDirection) {
    const current = session;
    if (!current) return;
    const ordered = current.direction === "up" ? [...current.trail].reverse() : current.trail;
    const target = direction === "up" ? ordered[0] : ordered.at(-1)!;
    clear();
    const token = revision;
    await pages.get(target.pageId)?.focusEdge(target.blockId, current.x, direction === "up" ? "top" : "bottom",
      () => !destroyed && token === revision && !!scope()?.contains(document.activeElement));
  }

  function usable(event: Event) {
    return !!session && !hasKeyboardOverlay(document) && !!scope()?.contains(event.target as Node)
      && !(event.target as Element)?.closest?.("input, textarea, select, [contenteditable='true']");
  }

  function onKeydown(event: KeyboardEvent) {
    const waitingForHistory = busy && (event.ctrlKey || event.metaKey) && ["z", "y"].includes(event.key.toLowerCase());
    if (!waitingForHistory && !["Shift", "Control", "Meta", "Alt", "Delete", "Backspace"].includes(event.key)) deletionFocus = null;
    if (event.isComposing || event.keyCode === 229) return;
    if (pending && event.key !== "Shift"
      && !(event.shiftKey && !event.ctrlKey && !event.metaKey && !event.altKey
        && event.key === (pending.direction === "up" ? "ArrowUp" : "ArrowDown"))) {
      revision += 1;
      pending = null;
    }
    if (!usable(event)) return;
    let action: (() => void | Promise<void>) | undefined;
    if ((event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey && event.key.toLowerCase() === "x") {
      action = async () => {
        const current = session;
        const token = revision;
        await writeClipboardText(payload().markdown);
        if (session === current && revision === token) await remove();
      };
    }
    if (!event.ctrlKey && !event.metaKey && !event.altKey) {
      if (event.key === "ArrowUp" || event.key === "ArrowDown") {
        const direction = event.key === "ArrowUp" ? "up" : "down";
        action = () => event.shiftKey ? extend(direction) : collapseToEdge(direction);
      } else if (event.key === "Escape") {
        action = () => restore(true);
      } else if (event.key === "Delete" || event.key === "Backspace") {
        action = remove;
      } else if (event.key === "Tab" || event.key === "ISO_Left_Tab" || event.code === "Tab") {
        action = async () => {
          revision += 1;
          for (const [pageId] of groups()) await pages.get(pageId)?.indent(event.shiftKey ? "out" : "in");
        };
      }
    }
    if (!action) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    if (!busy) void Promise.resolve(action()).catch(report);
  }

  function onClipboard(event: ClipboardEvent) {
    if (!usable(event) || busy || pending) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    if (!event.clipboardData) {
      report(new Error("The clipboard is unavailable"));
      return;
    }
    try {
      const data = payload();
      event.clipboardData.setData("text/plain", data.plainText);
      event.clipboardData.setData("text/markdown", data.markdown);
      if (event.type === "cut") void remove().catch(report);
    } catch (error) {
      report(error);
    }
  }

  function onPointerdown(event: PointerEvent) {
    deletionFocus = null;
    revision += 1;
    pending = null;
    if (session && !busy && !(event.target as Element)?.closest?.(".keyboard-selection-toolbar")) clear();
  }

  function onInput() { revision += 1; pending = null; deletionFocus = null; }
  function onFocusin(event: FocusEvent) {
    const request = deletionFocus;
    if (request && event.target !== request.source
      && !(event.target === document.body && (request.toolbar || !request.source.isConnected))) {
      deletionFocus = null;
    }
  }
  function onAppHistory(event: Event) {
    if (!busy) return;
    event.stopImmediatePropagation();
    showToast("Finishing the block deletion. Please wait before undoing or redoing.", "info");
  }
  window.addEventListener("keydown", onKeydown, true);
  window.addEventListener("copy", onClipboard, true);
  window.addEventListener("cut", onClipboard, true);
  window.addEventListener("pointerdown", onPointerdown, true);
  window.addEventListener("input", onInput, true);
  window.addEventListener("focusin", onFocusin, true);
  window.addEventListener("app-undo", onAppHistory, true);
  window.addEventListener("app-redo", onAppHistory, true);

  return {
    get active() { return session !== null; },
    get busy() { return busy; },
    get count() { return session?.trail.length ?? 0; },
    get pageCount() { return new Set(session?.trail.map((point) => point.pageId)).size; },
    begin,
    clear,
    copy: () => void copy().catch(report),
    remove: () => void remove().catch(report),
    register(pageId: string, page: SelectionPage) {
      pages.set(pageId, page);
      return () => {
        if (highlighted.has(pageId)) clear();
        if (pages.get(pageId) === page) pages.delete(pageId);
      };
    },
    destroy() {
      destroyed = true;
      clear();
      window.removeEventListener("keydown", onKeydown, true);
      window.removeEventListener("copy", onClipboard, true);
      window.removeEventListener("cut", onClipboard, true);
      window.removeEventListener("pointerdown", onPointerdown, true);
      window.removeEventListener("input", onInput, true);
      window.removeEventListener("focusin", onFocusin, true);
      window.removeEventListener("app-undo", onAppHistory, true);
      window.removeEventListener("app-redo", onAppHistory, true);
    },
  };
}

export type KeyboardBlockSelection = ReturnType<typeof createKeyboardBlockSelection>;
