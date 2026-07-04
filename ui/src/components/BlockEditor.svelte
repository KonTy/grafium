<script lang="ts">
  import { onMount, tick } from "svelte";
  import { EditorView, keymap, placeholder as cmPlaceholder, lineNumbers } from "@codemirror/view";
  import { EditorState, EditorSelection } from "@codemirror/state";
  import { defaultKeymap, indentWithTab, history, historyKeymap, undo, redo } from "@codemirror/commands";
  import { autocompletion, startCompletion, completionStatus, type CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
  import { markdown } from "@codemirror/lang-markdown";
  import { renderBlock, hydrateAssetMedia } from "../lib/markdown";
  import { updateBlock, createBlock, deleteBlock, runQuery, cycleTaskState, getBlockPageTitle, setTaskDate, downloadAsset } from "../lib/api";
  import type { QueryRow } from "../lib/api";
  import { keymap_manager } from "../lib/keymap";
  import { htmlToMarkdown, splitMarkdownIntoBlocks, localizeImages } from "../lib/htmlToMd";
  import { buildSaveContext, persistBlockContentIfChanged } from "../lib/persistence";
  import type { PasteBlock } from "../lib/htmlToMd";
  import type { Block } from "../lib/api";
  import DatePicker from "./DatePicker.svelte";

  interface Props {
    block: Block;
    pageId: string;
    pageTitle?: string;
    depth?: number;
    focused?: boolean;
    selected?: boolean;
    hasChildren?: boolean;
    collapsed?: boolean;
    onFocus?: (blockId: string) => void;
    onBlur?: (blockId: string) => void;
    onEnter?: (blockId: string, content: string, orderIndex: number, atStart: boolean) => void;
    onDelete?: (blockId: string) => void;
    onIndent?: (blockId: string, direction: "in" | "out", currentContent?: string) => void;
    onNavigate?: (blockId: string, direction: "up" | "down", caretX?: number) => void;
    onBulletClick?: (blockId: string, event: MouseEvent) => void;
    onPasteBlocks?: (blockId: string, blocks: PasteBlock[]) => void;
    onToggleCollapse?: (blockId: string) => void;
  }

  let {
    block,
    pageId,
    pageTitle = "",
    depth = 0,
    focused = false,
    selected = false,
    hasChildren = false,
    collapsed = false,
    onFocus,
    onBlur,
    onEnter,
    onDelete,
    onIndent,
    onNavigate,
    onBulletClick,
    onPasteBlocks,
    onToggleCollapse,
  }: Props = $props();

  let editorContainer: HTMLDivElement;
  let editorView: EditorView | undefined;
  let savedState: EditorState | undefined;
  let blurTeardownTimer: number | undefined;
  let shiftHeld = false;
  let isEditing = $state(false);
  let isCodeBlock = $derived(detectCodeBlock(block.content));
  let renderedHtml = $derived(renderBlock(block.content));

  // Rendered-content container, used to hydrate <audio>/<video> media that
  // WebKitGTK can't load from the custom asset scheme.
  let renderedEl = $state<HTMLElement | null>(null);
  $effect(() => {
    void renderedHtml;
    const el = renderedEl;
    queueMicrotask(() => hydrateAssetMedia(el));
  });

  // Date picker state
  let showDatePicker = $state(false);
  let datePickerKind: "scheduled" | "deadline" = $state("scheduled");
  let datePickerPos = $state({ x: 0, y: 0 });

  // Query block support
  const QUERY_RE = /^\{\{query\s+([\s\S]+?)\}\}\s*$/;
  let queryExpression = $derived((() => {
    const m = block.content.trim().match(QUERY_RE);
    return m ? m[1].trim() : null;
  })());
  let queryRows: QueryRow[] | null = $state(null);
  let queryColumns: string[] = $state([]);
  let queryError: string | null = $state(null);
  let queryLoading = $state(false);
  let queryBlockIdCol = $state(-1);
  let bulletMinHeight = $derived(getBulletMinHeight(block.content));
  let editorStyleClass = $derived(getEditorStyleClass(block.content));
  let isQuoteBlock = $derived(block.content.trimStart().startsWith(">"));

  const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

  async function runQueryBlock(expr: string) {
    queryLoading = true;
    queryError = null;
    try {
      let rows = await runQuery(expr);
      // Exclude query blocks from results (blocks starting with {{query)
      rows = rows.filter((row) => {
        return !row.some(([col, val]) => {
          if (typeof val !== "string") return false;
          // Exclude if this row is the query block itself (by id)
          if (col.toLowerCase() === "_block_id" && val === block.id) return true;
          // Exclude if any cell contains a query block
          if (val.trimStart().startsWith("{{query")) return true;
          return false;
        });
      });
      queryRows = rows;
      queryColumns = rows.length > 0 ? rows[0].map(([col]) => col) : [];
      // Find the block id column (id, block_id, _block_id)
      const lowerCols = queryColumns.map((c) => c.toLowerCase());
      queryBlockIdCol = lowerCols.indexOf("_block_id");
      if (queryBlockIdCol < 0) queryBlockIdCol = lowerCols.indexOf("id");
      if (queryBlockIdCol < 0) queryBlockIdCol = lowerCols.indexOf("block_id");
      // Fallback: find first column where all values look like UUIDs
      if (queryBlockIdCol < 0 && rows.length > 0) {
        for (let i = 0; i < queryColumns.length; i++) {
          const allUuid = rows.every((row) => {
            const val = row[i]?.[1];
            return typeof val === "string" && UUID_RE.test(val);
          });
          if (allUuid) { queryBlockIdCol = i; break; }
        }
      }
    } catch (e: unknown) {
      queryError = e instanceof Error ? e.message : String(e);
      queryRows = [];
      queryColumns = [];
      queryBlockIdCol = -1;
    } finally {
      queryLoading = false;
    }
  }

  // Run query when not editing and it's a query block
  $effect(() => {
    if (queryExpression && !isEditing) {
      runQueryBlock(queryExpression);
    }
  });

  // Slash command completion source
  type SlashCommand = {
    label: string;
    detail: string;
    apply: string;
    cursorOffset?: number;
    action?: string; // "scheduled" | "deadline"
  };

  const SLASH_COMMANDS: SlashCommand[] = [
    {
      label: "/query",
      detail: "Run a SQL SELECT and display results",
      apply: "{{query SELECT }}",
      cursorOffset: 15,
    },
    {
      label: "/TODO",
      detail: "Insert a TODO task marker",
      apply: "TODO ",
    },
    {
      label: "/DONE",
      detail: "Insert a DONE task marker",
      apply: "DONE ",
    },
    {
      label: "/DOING",
      detail: "Insert a DOING task marker",
      apply: "DOING ",
    },
    {
      label: "/NOW",
      detail: "Insert a NOW task marker",
      apply: "NOW ",
    },
    {
      label: "/LATER",
      detail: "Insert a LATER task marker",
      apply: "LATER ",
    },
    {
      label: "/CANCELED",
      detail: "Mark task as canceled",
      apply: "CANCELED ",
    },
    {
      label: "/Scheduled",
      detail: "Set a scheduled date for this task",
      apply: "",
      action: "scheduled",
    },
    {
      label: "/Deadline",
      detail: "Set a deadline date for this task",
      apply: "",
      action: "deadline",
    },
    {
      label: "/Priority A",
      detail: "Set priority A (highest)",
      apply: "[#A] ",
    },
    {
      label: "/Priority B",
      detail: "Set priority B (medium)",
      apply: "[#B] ",
    },
    {
      label: "/Priority C",
      detail: "Set priority C (low)",
      apply: "[#C] ",
    },
  ];

  function slashCompletionSource(context: CompletionContext): CompletionResult | null {
    // Match a `/` optionally followed by word chars at the current position
    const match = context.matchBefore(/\/[^\s]*/);
    if (!match) return null;

    return {
      from: match.from,
      filter: false,
      options: SLASH_COMMANDS.map((cmd) => ({
        label: cmd.label,
        detail: cmd.detail,
        apply: (view: EditorView, _completion: unknown, from: number, to: number) => {
          if (cmd.action) {
            // Remove the slash command text
            view.dispatch({
              changes: { from, to, insert: "" },
            });
            // Show the date picker
            const coords = view.coordsAtPos(from);
            datePickerKind = cmd.action as "scheduled" | "deadline";
            datePickerPos = { x: coords?.left ?? 100, y: (coords?.bottom ?? 100) + 4 };
            showDatePicker = true;
          } else {
            view.dispatch({
              changes: { from, to, insert: cmd.apply },
              selection: EditorSelection.cursor(
                from + (cmd.cursorOffset ?? cmd.apply.length)
              ),
            });
          }
        },
      })),
    };
  }

  // Detect if the block is entirely a code fence
  function detectCodeBlock(content: string): { lang: string; code: string } | null {
    const trimmed = content.trim();
    if (!trimmed.startsWith("```") || !trimmed.endsWith("```")) return null;
    const firstNewline = trimmed.indexOf("\n");
    if (firstNewline === -1) return null;
    const lastNewline = trimmed.lastIndexOf("\n");
    const lang = trimmed.slice(3, firstNewline).trim();
    if (firstNewline === lastNewline) {
      // ```lang\n``` — empty code block
      return { lang, code: "" };
    }
    const code = trimmed.slice(firstNewline + 1, lastNewline);
    return { lang, code };
  }

  function getHeadingLevel(content: string): number {
    const trimmed = content.trimStart();
    const match = trimmed.match(/^(#{1,6})\s+/);
    return match ? match[1].length : 0;
  }

  function getBulletMinHeight(content: string): string {
    switch (getHeadingLevel(content)) {
      case 1:
        return "2.7em";
      case 2:
        return "2.25em";
      case 3:
        return "1.875em";
      case 4:
      case 5:
      case 6:
        return "1.65em";
      default:
        return "24px";
    }
  }

  function getEditorStyleClass(content: string): string {
    const level = getHeadingLevel(content);
    if (level > 0) return `h${level}`;
    return content.includes("\n") ? "multiline-block" : "normal-block";
  }

  function normalizeTaskPrefix(content: string): string {
    return content.replace(
      /^(todo|doing|done|later|now|canceled)\s+/i,
      (match, keyword: string) => `${keyword.toUpperCase()} `
    );
  }

  // Save content on blur
  async function saveContent(content: string) {
    const context = buildSaveContext(block.id, pageId, block.content, content);
    console.log("[telemetry] savecontext", JSON.stringify(context));
    const changed = await persistBlockContentIfChanged(block, content, (id, value) => updateBlock(id, value));
    if (changed) {
      console.log("[telemetry] saveContent", JSON.stringify(context));
    }
  }

  // Detect if cursor is inside a code fence (``` ... ```)
  function isInsideCodeFence(view: EditorView): boolean {
    const doc = view.state.doc.toString();
    const pos = view.state.selection.main.head;
    const lines = doc.split("\n");
    let charCount = 0;
    let insideFence = false;

    for (const line of lines) {
      if (line.trimStart().startsWith("```")) {
        if (insideFence) {
          // Closing fence — check if cursor is before it
          if (pos <= charCount + line.length) return insideFence;
          insideFence = false;
        } else {
          // Opening fence — check if cursor is after it
          insideFence = pos > charCount + line.length;
        }
      }
      charCount += line.length + 1; // +1 for newline
    }
    return insideFence;
  }

  // Imperative caret target for cross-block Arrow Up/Down navigation. Set by
  // the parent via focusForNav() right before/while the editor opens.
  let navPending: { x: number; edge: "top" | "bottom" } | null = null;

  /** Called imperatively by the parent (PageContent) when this block is the
   *  target of an Arrow Up/Down move. Opens the editor and places the caret at
   *  viewport-x `x` on the top or bottom visual line. Deterministic — does not
   *  depend on synthetic clicks or prop-propagation timing (both unreliable on
   *  WebKitGTK). */
  export function focusForNav(x: number, edge: "top" | "bottom") {
    navPending = { x, edge };
    (window as any).__keydbg?.(`FOCUSNAV ${block.id.slice(0, 4)} ${edge} ed=${isEditing}`);
    if (isEditing && editorView) {
      placeNavCaret(editorView);
    } else {
      startEditing();
    }
  }

  function placeNavCaret(view: EditorView) {
    if (!navPending) return;
    const { x, edge } = navPending;
    navPending = null;
    const anchor = edge === "top"
      ? view.coordsAtPos(0)
      : view.coordsAtPos(view.state.doc.length);
    if (!anchor) return;
    const y = edge === "top" ? anchor.top + 2 : anchor.bottom - 2;
    const pos = view.posAtCoords({ x, y });
    if (pos != null) {
      view.dispatch({ selection: EditorSelection.cursor(pos) });
    } else if (edge === "bottom") {
      view.dispatch({ selection: EditorSelection.cursor(view.state.doc.length) });
    }
    view.focus();
  }

  function startEditing() {
    if (isEditing) return;
    isEditing = true;
    keymap_manager.isEditing = true;
    onFocus?.(block.id);

    // Open the editor as soon as the container is in the DOM. `tick()` flushes
    // Svelte's pending DOM update in a microtask, so on the common path the
    // editor appears well before the next paint frame — much snappier for
    // cross-block Arrow Up/Down navigation than waiting a full rAF. Fall back
    // to an rAF retry only if the container somehow isn't ready yet.
    tick().then(() => {
      if (editorContainer) {
        initEditor();
        return;
      }
      const tryInit = (attempts: number) => {
        requestAnimationFrame(() => {
          if (!editorContainer) {
            if (attempts < 8) tryInit(attempts + 1);
            return;
          }
          initEditor();
        });
      };
      tryInit(0);
    });
  }

  function initEditor() {
      // Reuse saved state if content hasn't changed externally
      let state: EditorState;
      if (savedState && savedState.doc.toString() === block.content) {
        state = savedState;
      } else {
        state = EditorState.create({
        doc: block.content,
        extensions: [
          markdown(),
          history(),
          autocompletion({
            override: [slashCompletionSource],
            activateOnTyping: false,
            closeOnBlur: false,
          }),
          keymap.of([
            {
              key: "/",
              run: (view) => {
                if (isInsideCodeFence(view)) {
                  return false;
                }
                const { from, to } = view.state.selection.main;
                view.dispatch({
                  changes: { from, to, insert: "/" },
                  selection: EditorSelection.cursor(from + 1),
                });
                startCompletion(view);
                return true;
              },
            },
            {
              key: "Shift-/",
              run: (view) => {
                if (isInsideCodeFence(view)) {
                  return false;
                }
                const { from, to } = view.state.selection.main;
                view.dispatch({
                  changes: { from, to, insert: "/" },
                  selection: EditorSelection.cursor(from + 1),
                });
                startCompletion(view);
                return true;
              },
            },
            {
              key: "Mod-z",
              run: (view) => undo(view),
            },
            {
              key: "Mod-Shift-z",
              run: (view) => redo(view),
            },
            {
              key: "Mod-y",
              run: (view) => redo(view),
            },
            {
              key: "Enter",
              run: (view) => {
                // If inside a code fence, insert a newline instead
                if (isInsideCodeFence(view)) {
                  const { from } = view.state.selection.main;
                  view.dispatch({
                    changes: { from, to: from, insert: "\n" },
                    selection: EditorSelection.cursor(from + 1),
                  });
                  return true;
                }
                const content = normalizeTaskPrefix(view.state.doc.toString());
                if (content !== view.state.doc.toString()) {
                  view.dispatch({
                    changes: { from: 0, to: view.state.doc.length, insert: content },
                  });
                }
                const sel = view.state.selection.main;
                const atStart = sel.from === 0 && sel.to === 0;
                onEnter?.(block.id, content, block.order_index, atStart);
                return true;
              },
            },
            {
              key: "Backspace",
              run: (view) => {
                if (view.state.doc.length === 0) {
                  onDelete?.(block.id);
                  return true;
                }
                return false;
              },
            },
            {
              key: "Delete",
              run: (view) => {
                if (view.state.doc.length === 0) {
                  onDelete?.(block.id);
                  return true;
                }
                return false;
              },
            },
            {
              key: "Tab",
              run: (view) => {
                // Inside code fence: insert tab/spaces
                if (isInsideCodeFence(view)) {
                  const { from } = view.state.selection.main;
                  view.dispatch({
                    changes: { from, to: from, insert: "  " },
                    selection: EditorSelection.cursor(from + 2),
                  });
                  return true;
                }
                const content = view.state.doc.toString();
                onIndent?.(block.id, "in", content);
                return true;
              },
            },
            {
              key: "Shift-Tab",
              run: (view) => {
                if (isInsideCodeFence(view)) {
                  return false; // let default handle dedent
                }
                const content = view.state.doc.toString();
                onIndent?.(block.id, "out", content);
                return true;
              },
            },
            {
              key: "Escape",
              run: (view) => {
                const content = view.state.doc.toString();
                saveContent(content);
                stopEditing();
                return true;
              },
            },
            ...defaultKeymap,
            ...historyKeymap,
            indentWithTab,
          ]),
          EditorView.lineWrapping,
          EditorView.theme({
            "&": {
              fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', sans-serif",
              fontSize: "inherit",
              lineHeight: "inherit",
              fontWeight: "inherit",
            },
            "&.cm-editor": {
              padding: "0",
            },
            ".cm-scroller": {
              padding: "0",
              overflow: "visible",
              fontFamily: "inherit",
            },
            ".cm-content": {
              padding: "0",
              caretColor: "var(--text-primary)",
              color: "var(--text-primary)",
              minHeight: "auto",
              fontFamily: "inherit",
              fontSize: "inherit",
              lineHeight: "inherit",
              fontWeight: "inherit",
            },
            "&.cm-focused": {
              outline: "none",
            },
            ".cm-line": {
              padding: "0",
              lineHeight: "inherit",
              fontFamily: "inherit",
              fontSize: "inherit",
              fontWeight: "inherit",
            },
            ".cm-cursor": {
              borderLeftColor: "var(--text-primary)",
            },
            ".cm-gutters": {
              display: "none",
            },
          }),
          EditorView.updateListener.of((update) => {
            if (!update.view.hasFocus) return;
            const sel = update.state.selection.main;
            if (!sel.empty) return;

            const line = update.state.doc.lineAt(sel.head);
            const beforeCursor = line.text.slice(0, sel.head - line.from);
            const slashToken = beforeCursor.match(/(?:^|\s)\/[^\s]*$/);
            if (!slashToken) return;

            const status = completionStatus(update.state);
            if (status === null) {
              startCompletion(update.view);
            }
          }),
          EditorView.domEventHandlers({
            focus: () => {
              if (blurTeardownTimer !== undefined) {
                window.clearTimeout(blurTeardownTimer);
                blurTeardownTimer = undefined;
              }
            },
            keydown: (event) => {
              if (event.key === "Shift") shiftHeld = true;
            },
            keyup: (event) => {
              if (event.key === "Shift") shiftHeld = false;
            },
            paste: (event, view) => {
              const html = event.clipboardData?.getData("text/html");
              if (!html) return false;
              event.preventDefault();
              const md = htmlToMarkdown(html);

              if (shiftHeld || !onPasteBlocks) {
                // Ctrl+Shift+V: paste everything into this one block
                const { from, to } = view.state.selection.main;
                view.dispatch({
                  changes: { from, to, insert: md },
                  selection: EditorSelection.cursor(from + md.length),
                });
                // Download images in background and update content
                localizeImages(md, downloadAsset).then((localized) => {
                  if (localized !== md) {
                    const doc = view.state.doc.toString();
                    const updated = doc.replace(md, localized);
                    view.dispatch({ changes: { from: 0, to: doc.length, insert: updated } });
                    saveContent(updated);
                  }
                });
              } else {
                // Ctrl+V: split into separate blocks with hierarchy
                const chunks = splitMarkdownIntoBlocks(md);
                // First chunk goes into the current block at cursor
                const { from, to } = view.state.selection.main;
                view.dispatch({
                  changes: { from, to, insert: chunks[0].content },
                  selection: EditorSelection.cursor(from + chunks[0].content.length),
                });
                // Remaining chunks become new blocks (with depth info)
                if (chunks.length > 1) {
                  const content = view.state.doc.toString();
                  saveContent(content);
                  block.content = content;
                  onPasteBlocks(block.id, chunks.slice(1));
                }
                // Download images in all pasted blocks in background
                localizeImages(md, downloadAsset).then((localized) => {
                  if (localized !== md) {
                    // Re-split and update the first block
                    const localChunks = splitMarkdownIntoBlocks(localized);
                    if (localChunks[0]?.content !== chunks[0]?.content) {
                      const newContent = view.state.doc.toString().replace(chunks[0].content, localChunks[0].content);
                      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: newContent } });
                      saveContent(newContent);
                    }
                  }
                });
              }
              return true;
            },
            blur: (_event, view) => {
              // WebKitGTK can fire blur before focus settles on autocomplete/editor DOM.
              // Debounce teardown and cancel if focus returns to editor context.
              if (blurTeardownTimer !== undefined) {
                window.clearTimeout(blurTeardownTimer);
              }
              blurTeardownTimer = window.setTimeout(() => {
                if (!editorView || editorView !== view) return;

                const completionState = completionStatus(view.state);
                if (completionState === "active" || completionState === "pending") {
                  view.focus();
                  return;
                }

                const active = document.activeElement as HTMLElement | null;
                // Only keep THIS editor alive if focus is still within it (e.g.
                // its own autocomplete popup). If focus moved to a DIFFERENT
                // block's editor (cross-block navigation), tear this one down so
                // it re-renders as markdown.
                const stillInThisEditor = !!active && view.dom.contains(active);
                const inAutocomplete = !!active?.closest(".cm-tooltip-autocomplete");
                if (stillInThisEditor || inAutocomplete) {
                  return;
                }

                const content = view.state.doc.toString();
                saveContent(content);
                savedState = view.state;
                (window as any).__activeEditorView = undefined;
                editorView?.destroy();
                editorView = undefined;
                isEditing = false;
                keymap_manager.isEditing = false;
                onBlur?.(block.id);
                blurTeardownTimer = undefined;
              }, 120);
            },
          }),
          // Auto-close ``` into a code fence
          EditorView.inputHandler.of((view, from, to, text) => {
            if (text === "`") {
              const doc = view.state.doc.toString();
              const before = doc.slice(0, from);
              // Check if this completes "```" at the start of a line
              if (before.endsWith("``") && (before.length === 2 || before[before.length - 3] === "\n")) {
                const fenceStart = from - 2;
                view.dispatch({
                  changes: { from: fenceStart, to, insert: "```\n\n```" },
                  selection: EditorSelection.cursor(fenceStart + 4),
                });
                return true;
              }
            }
            return false;
          }),
        ],
      });
      }

      editorView = new EditorView({ state, parent: editorContainer });
      (window as any).__activeEditorView = editorView;
      editorView.focus();

      // WebKitGTK note: arrow keydown events are consumed by native
      // contentEditable caret movement and do NOT reliably reach CodeMirror's
      // keymap — AND native vertical movement between wrapped visual rows is
      // also unreliable. So we own vertical movement entirely: intercept Arrow
      // Up/Down in the CAPTURE phase and move the caret ourselves, preserving
      // the visual column (x). Only when already on the first/last visual row
      // do we cross into the adjacent block.
      {
        const view = editorView;
        const onArrowKey = (e: KeyboardEvent) => {
          if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
          if (e.shiftKey || e.altKey || e.ctrlKey || e.metaKey) return;
          if (!editorView || editorView !== view) return;
          // If the slash/autocomplete popup is open, let CodeMirror handle
          // Up/Down to move the menu selection instead of moving the caret.
          const cstatus = completionStatus(view.state);
          if (cstatus === "active" || cstatus === "pending") return;
          const sel = view.state.selection.main;
          if (!sel.empty) return; // let native handle shift-selection etc.
          const caret = view.coordsAtPos(sel.head);
          if (!caret) return;
          const x = caret.left;
          const h = Math.max(4, caret.bottom - caret.top);
          if (e.key === "ArrowUp") {
            const top = view.coordsAtPos(0);
            const onFirstRow = !top || caret.top - top.top <= 1;
            e.preventDefault();
            e.stopImmediatePropagation();
            if (onFirstRow) {
              onNavigate?.(block.id, "up", x);
            } else {
              const pos = view.posAtCoords({ x, y: caret.top - h / 2 });
              if (pos != null) view.dispatch({ selection: EditorSelection.cursor(pos) });
            }
          } else {
            const end = view.coordsAtPos(view.state.doc.length);
            const onLastRow = !end || end.bottom - caret.bottom <= 1;
            e.preventDefault();
            e.stopImmediatePropagation();
            if (onLastRow) {
              onNavigate?.(block.id, "down", x);
            } else {
              const pos = view.posAtCoords({ x, y: caret.bottom + h / 2 });
              if (pos != null) view.dispatch({ selection: EditorSelection.cursor(pos) });
            }
          }
        };
        view.contentDOM.addEventListener("keydown", onArrowKey, true);
      }

      // Column-preserving vertical navigation: when this block was reached by
      // pressing Arrow Up/Down in an adjacent block, drop the caret at the same
      // viewport x on the appropriate (top/bottom) visual line — like MS Word.
      placeNavCaret(editorView);
  }

  function stopEditing() {
    if (blurTeardownTimer !== undefined) {
      window.clearTimeout(blurTeardownTimer);
      blurTeardownTimer = undefined;
    }
    if (editorView) {
      const content = editorView.state.doc.toString();
      saveContent(content);
      editorView.destroy();
      editorView = undefined;
    }
    isEditing = false;
    keymap_manager.isEditing = false;

    // Ensure focus does not remain on a stale contenteditable node.
    requestAnimationFrame(() => {
      const active = document.activeElement as HTMLElement | null;
      if (!active) return;
      if (active.isContentEditable || active.closest(".cm-editor")) {
        active.blur();
      }
    });
  }

  // Clean up
  $effect(() => {
    return () => {
      if (blurTeardownTimer !== undefined) {
        window.clearTimeout(blurTeardownTimer);
        blurTeardownTimer = undefined;
      }
      if (editorView) {
        editorView.destroy();
      }
    };
  });

  function handleClick() {
    if (!isEditing && !queryExpression) {
      startEditing();
    }
  }

  async function handleTaskCycle() {
    try {
      const newState = await cycleTaskState(block.id);
      // Backend already updated block content + .md file;
      // update local state to match
      const taskRe = /^(TODO|DOING|DONE|NOW|LATER|CANCELED)\s/;
      const newContent = block.content.replace(taskRe, newState + " ");
      if (newContent !== block.content) {
        block.content = newContent;
      }
    } catch (e) {
      console.error("Failed to cycle task state:", e);
    }
  }

  async function handleQueryResultClick(e: MouseEvent) {
    const target = e.target as HTMLElement;

    // Handle task-marker clicks inside query results
    if (target.classList.contains("task-marker")) {
      e.stopPropagation();
      e.preventDefault();
      if (queryBlockIdCol < 0) return;
      // Find the row index
      const row = target.closest("tr");
      if (!row) return;
      const tbody = row.closest("tbody");
      if (!tbody) return;
      const rowIdx = Array.from(tbody.children).indexOf(row);
      if (rowIdx < 0 || !queryRows || rowIdx >= queryRows.length) return;
      const blockId = String(queryRows[rowIdx][queryBlockIdCol][1] ?? "");
      if (!blockId) return;
      try {
        await cycleTaskState(blockId);
        // Re-run the query to refresh results
        if (queryExpression) {
          await runQueryBlock(queryExpression);
        }
      } catch (err) {
        console.error("Failed to cycle task in query result:", err);
      }
      return;
    }

    // Handle page-link clicks inside query results
    if (target.classList.contains("page-link")) {
      e.stopPropagation();
      const pageName = target.dataset.page;
      if (pageName) {
        window.dispatchEvent(new CustomEvent("navigate-page", { detail: { pageName } }));
      }
      return;
    }

    // Handle tag clicks
    if (target.classList.contains("tag")) {
      e.stopPropagation();
      const tag = target.dataset.tag;
      if (tag) {
        window.dispatchEvent(new CustomEvent("navigate-page", { detail: { pageName: tag } }));
      }
      return;
    }

    // Handle normal text clicks — navigate to the block's page
    if (queryBlockIdCol >= 0) {
      const row = target.closest("tr");
      if (!row) return;
      const tbody = row.closest("tbody");
      if (!tbody) return;
      const rowIdx = Array.from(tbody.children).indexOf(row);
      if (rowIdx < 0 || !queryRows || rowIdx >= queryRows.length) return;
      const blockId = String(queryRows[rowIdx][queryBlockIdCol][1] ?? "");
      if (!blockId) return;
      e.stopPropagation();
      try {
        const pageTitle = await getBlockPageTitle(blockId);
        window.dispatchEvent(new CustomEvent("navigate-page", {
          detail: { pageName: pageTitle, targetBlockId: blockId }
        }));
      } catch (err) {
        console.error("Failed to navigate to block:", err);
      }
    }
  }

  function handleRenderedClick(e: MouseEvent) {
    const target = e.target as HTMLElement;

    // Handle task marker clicks — cycle state
    if (target.classList.contains("task-marker")) {
      e.stopPropagation();
      e.preventDefault();
      handleTaskCycle();
      return;
    }

    // Handle page link clicks
    if (target.classList.contains("page-link")) {
      e.stopPropagation();
      const pageName = target.dataset.page;
      if (pageName) {
        window.dispatchEvent(new CustomEvent("navigate-page", {
          detail: { pageName, sourceBlockId: block.id, sourcePageTitle: pageTitle },
        }));
      }
      return;
    }

    // Handle tag clicks
    if (target.classList.contains("tag")) {
      e.stopPropagation();
      const tag = target.dataset.tag;
      if (tag) {
        window.dispatchEvent(new CustomEvent("navigate-page", {
          detail: { pageName: tag, sourceBlockId: block.id, sourcePageTitle: pageTitle },
        }));
      }
      return;
    }

    startEditing();
  }

  async function handleDateSelect(date: string) {
    showDatePicker = false;
    try {
      const newContent = await setTaskDate(block.id, datePickerKind, date || null);
      // Update the editor if open
      if (editorView) {
        editorView.dispatch({
          changes: { from: 0, to: editorView.state.doc.length, insert: newContent },
        });
      }
      // Update the block reactive data
      block.content = newContent;
    } catch (e) {
      console.error("Failed to set task date:", e);
    }
  }

  function handleDateCancel() {
    showDatePicker = false;
  }
</script>

<div
  class="block-item"
  class:editing={isEditing}
  class:selected
  class:code-block={isCodeBlock !== null}
  style="padding-left: {depth * 24}px"
>
  {#if !block.content.trim().startsWith("```") && !queryExpression && block.content.trim() !== "" && !isQuoteBlock}
    <div class="bullet-container" class:has-children={hasChildren} style={`min-height: ${bulletMinHeight};`} onclick={(e) => {
      e.stopPropagation();
      if (hasChildren) {
        onToggleCollapse?.(block.id);
      } else {
        onBulletClick?.(block.id, e);
      }
    }}>
      {#if hasChildren}
        <span class="collapse-arrow" class:collapsed>
          {#if collapsed}▶{:else}
            <span class="arrow-hover">▼</span><span class="bullet-default">•</span>
          {/if}
        </span>
      {:else}
        <span class="bullet">•</span>
      {/if}
    </div>
  {/if}
  <div class="block-content" class:quote-block={isQuoteBlock && !isEditing} onclick={handleClick}>
    {#if isEditing}
      <div class="editor-wrapper" class:normal-block={editorStyleClass === "normal-block"} class:multiline-block={editorStyleClass === "multiline-block"} class:h1={editorStyleClass === "h1"} class:h2={editorStyleClass === "h2"} class:h3={editorStyleClass === "h3"} class:h4={editorStyleClass === "h4"} class:h5={editorStyleClass === "h5"} class:h6={editorStyleClass === "h6"} bind:this={editorContainer}></div>
    {:else if queryExpression !== null}
      <!-- Query block rendered view -->
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="query-block" onclick={(e) => e.stopPropagation()}>
        <div class="query-header">
          <button class="query-edit-btn" onclick={(e) => { e.stopPropagation(); startEditing(); }} title="Edit query">
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35"/></svg>
          </button>
          <span class="query-label">Query</span>
          <button class="query-refresh" onclick={(e) => { e.stopPropagation(); runQueryBlock(queryExpression!); }} title="Re-run query">↻</button>
        </div>
        {#if queryLoading}
          <div class="query-loading">Running…</div>
        {:else if queryError}
          <div class="query-error">Error: {queryError}</div>
        {:else if queryRows !== null && queryRows.length === 0}
          <div class="query-empty">No results.</div>
        {:else if queryRows !== null}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div class="query-table-wrap" onclick={handleQueryResultClick}>
            <table class="query-table">
              <thead>
                <tr>
                  {#each queryColumns as col, i}
                    {#if i !== queryBlockIdCol || col.toLowerCase() !== "_block_id"}
                      <th>{col}</th>
                    {/if}
                  {/each}
                </tr>
              </thead>
              <tbody>
                {#each queryRows as row}
                  <tr>
                    {#each row as [col, val], i}
                      {#if i !== queryBlockIdCol || col.toLowerCase() !== "_block_id"}
                        <td>
                          {#if col === "content" && val}
                            <span class="rendered-content query-cell-content">{@html renderBlock(String(val))}</span>
                          {:else if col === "state" && val}
                            <span class="rendered-content"><span class="task-marker {String(val).toLowerCase()}">{val}</span></span>
                          {:else}
                            {val === null ? "" : String(val)}
                          {/if}
                        </td>
                      {/if}
                    {/each}
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      </div>
    {:else}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="rendered-content" onclick={handleRenderedClick} bind:this={renderedEl}>
        {#if block.content.trim() === ""}
          <span class="placeholder">&nbsp;</span>
        {:else}
          {@html renderedHtml}
        {/if}
      </div>
    {/if}
  </div>
</div>

{#if showDatePicker}
  <DatePicker
    x={datePickerPos.x}
    y={datePickerPos.y}
    onSelect={handleDateSelect}
    onCancel={handleDateCancel}
  />
{/if}

<style>
  .block-item {
    display: flex;
    align-items: center;
    min-height: 24px;
    min-width: 0;
    border-radius: 4px;
    transition: background-color 0.1s;
    scroll-margin: 40px;
  }

  .block-item.editing {
    background: transparent;
    align-items: center;
  }

  .block-item.selected {
    background: var(--accent, #7c3aed);
    background: color-mix(in srgb, var(--accent, #7c3aed) 20%, transparent);
  }

  .bullet-container {
    width: 20px;
    min-height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    cursor: pointer;
  }

  .bullet {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-secondary);
    display: block;
    font-size: 0;
  }

  .collapse-arrow {
    font-size: 10px;
    color: var(--text-muted);
    user-select: none;
    line-height: 1;
  }

  .collapse-arrow .arrow-hover {
    display: none;
    font-size: 10px;
  }

  .collapse-arrow .bullet-default {
    display: inline;
    font-size: 18px;
    line-height: 1;
  }

  .block-item:hover .collapse-arrow .arrow-hover {
    display: inline;
  }

  .block-item:hover .collapse-arrow .bullet-default {
    display: none;
  }

  .collapse-arrow.collapsed {
    font-size: 10px;
    color: var(--text-secondary);
  }

  .block-content {
    flex: 1;
    min-height: 24px;
    min-width: 0;
    display: flex;
    align-items: flex-start;
    cursor: text;
    line-height: 1.45;
    overflow: hidden;
  }

  .block-item.editing .block-content {
    overflow: visible;
  }

  .block-content.quote-block {
    position: relative;
    padding-left: 12px;
    overflow: visible;
  }

  .block-content.quote-block::before {
    content: "";
    position: absolute;
    left: 0;
    top: -4px;
    bottom: -4px;
    width: 3px;
    background: var(--accent);
    pointer-events: none;
  }

  .editor-wrapper {
    width: 100%;
    min-width: 0;
    overflow: visible;
    font-size: inherit;
    font-weight: inherit;
    line-height: inherit;
  }

  .editor-wrapper :global(.cm-editor),
  .editor-wrapper :global(.cm-content),
  .editor-wrapper :global(.cm-line) {
    font-family: inherit;
    font-size: inherit;
    font-weight: inherit;
    line-height: inherit;
  }

  .editor-wrapper.normal-block :global(.cm-editor),
  .editor-wrapper.multiline-block :global(.cm-editor) {
    font-size: 15px;
    line-height: 1.45;
    font-weight: 400;
  }

  .editor-wrapper.h1 :global(.cm-editor) {
    font-size: 1.45em;
    line-height: 1.2;
    font-weight: 700;
  }

  .editor-wrapper.h2 :global(.cm-editor) {
    font-size: 1.25em;
    line-height: 1.2;
    font-weight: 600;
  }

  .editor-wrapper.h3 :global(.cm-editor) {
    font-size: 1.12em;
    line-height: 1.25;
    font-weight: 600;
  }

  .editor-wrapper.h4 :global(.cm-editor),
  .editor-wrapper.h5 :global(.cm-editor),
  .editor-wrapper.h6 :global(.cm-editor) {
    font-size: 1em;
    line-height: 1.25;
    font-weight: 600;
  }

  .rendered-content {
    width: 100%;
    min-width: 0;
    padding: 0;
    overflow-wrap: break-word;
    word-break: break-word;
  }

  .rendered-content :global(p) {
    margin: 0;
  }

  .placeholder {
    color: var(--text-muted);
    font-style: italic;
  }

  .rendered-content :global(.page-link) {
    color: var(--text-link);
    cursor: pointer;
    text-decoration: none;
    border-bottom: 1px solid transparent;
  }

  .rendered-content :global(.page-link:hover) {
    color: var(--text-link-hover);
    border-bottom-color: var(--text-link-hover);
  }

  .rendered-content :global(.tag) {
    color: var(--accent-secondary);
    cursor: pointer;
    text-decoration: none;
  }

  .rendered-content :global(.block-ref) {
    color: var(--text-secondary);
    border-bottom: 1px dashed var(--text-muted);
    cursor: pointer;
  }

  .rendered-content :global(.task-marker) {
    font-weight: 700;
    font-size: 12px;
    padding: 1px 4px;
    border-radius: 3px;
    margin-right: 4px;
    cursor: pointer;
    user-select: none;
    transition: opacity 0.15s;
  }

  .rendered-content :global(.task-marker:hover) {
    opacity: 0.7;
  }

  .rendered-content :global(.task-marker.todo) {
    background: var(--task-todo-bg);
    color: var(--task-todo-fg);
  }

  .rendered-content :global(.task-marker.doing) {
    background: var(--task-doing-bg);
    color: var(--task-doing-fg);
  }

  .rendered-content :global(.task-marker.done) {
    background: var(--task-done-bg);
    color: var(--task-done-fg);
  }

  .rendered-content :global(.task-marker.later) {
    background: var(--task-later-bg);
    color: var(--task-later-fg);
  }

  .rendered-content :global(.task-marker.now) {
    background: var(--task-doing-bg);
    color: var(--task-doing-fg);
  }

  .rendered-content :global(.task-marker.canceled) {
    background: rgba(150, 150, 150, 0.15);
    color: var(--text-muted);
    text-decoration: line-through;
  }

  .rendered-content :global(.priority) {
    font-size: 0.75rem;
    font-weight: 700;
    padding: 1px 4px;
    border-radius: 3px;
    margin-right: 2px;
  }

  .rendered-content :global(.priority-A) {
    background: rgba(255, 80, 80, 0.15);
    color: #ff5050;
  }

  .rendered-content :global(.priority-B) {
    background: rgba(255, 170, 0, 0.15);
    color: #ffaa00;
  }

  .rendered-content :global(.priority-C) {
    background: rgba(100, 180, 255, 0.15);
    color: #64b4ff;
  }

  .rendered-content :global(.task-date) {
    display: inline-block;
    font-size: 0.75rem;
    padding: 1px 6px;
    border-radius: 4px;
    margin-left: 4px;
    vertical-align: middle;
  }

  .rendered-content :global(.task-date.scheduled) {
    background: rgba(100, 180, 255, 0.1);
    color: var(--text-secondary);
  }

  .rendered-content :global(.task-date.deadline) {
    background: rgba(255, 100, 100, 0.1);
    color: #ff6464;
  }

  .rendered-content :global(code) {
    background: var(--bg-code);
    padding: 1px 4px;
    border-radius: 3px;
    font-size: 0.9em;
  }

  .rendered-content :global(strong) {
    font-weight: 700;
  }

  .rendered-content :global(a:not(.page-link):not(.tag)) {
    color: var(--text-link);
    text-decoration: underline;
  }

  .rendered-content :global(a:not(.page-link):not(.tag):hover) {
    color: var(--text-link-hover);
  }

  .rendered-content :global(h1) {
    font-size: 1.45em;
    font-weight: 700;
    line-height: 1.2;
    margin: 0;
  }

  .rendered-content :global(h2) {
    font-size: 1.25em;
    font-weight: 600;
    line-height: 1.2;
    margin: 0;
  }

  .rendered-content :global(h3) {
    font-size: 1.12em;
    font-weight: 600;
    line-height: 1.25;
    margin: 0;
  }

  .rendered-content :global(h4),
  .rendered-content :global(h5),
  .rendered-content :global(h6) {
    font-size: 1em;
    font-weight: 600;
    line-height: 1.25;
    margin: 0;
  }

  .rendered-content :global(blockquote) {
    border-left: 3px solid var(--accent);
    padding-left: 12px;
    margin: 0;
    min-height: 24px;
    display: flex;
    align-items: center;
    color: var(--text-secondary);
  }

  .rendered-content :global(blockquote > p) {
    margin: 0;
  }

  .block-content.quote-block .rendered-content :global(blockquote) {
    border-left: none;
    padding-left: 0;
  }

  .rendered-content :global(pre) {
    background: var(--bg-code);
    border-radius: 6px;
    padding: 12px;
    overflow-x: auto;
    margin: 4px 0;
  }

  .rendered-content :global(pre code) {
    background: none;
    padding: 0;
    font-size: 0.85em;
  }

  .rendered-content :global(.code-block-wrapper) {
    position: relative;
    background: var(--bg-code);
    border-radius: 6px;
    margin: 4px 0;
    overflow: hidden;
  }

  .rendered-content :global(.code-lang) {
    position: absolute;
    top: 4px;
    right: 8px;
    font-size: 11px;
    color: var(--text-muted);
    font-family: inherit;
  }

  .rendered-content :global(.code-block-pre) {
    margin: 0;
    padding: 10px 12px;
    background: none;
    border-radius: 0;
    overflow-x: auto;
    counter-reset: codeline;
  }

  .rendered-content :global(.code-block-pre code) {
    background: none;
    padding: 0;
    font-family: 'JetBrains Mono', 'Fira Code', monospace;
    font-size: 13px;
    line-height: 1.5;
  }

  .rendered-content :global(.code-line) {
    display: block;
    counter-increment: codeline;
  }

  .rendered-content :global(.code-line)::before {
    content: counter(codeline);
    display: inline-block;
    width: 2em;
    margin-right: 1em;
    text-align: right;
    color: var(--text-muted);
    user-select: none;
  }

  .rendered-content :global(.fc-img) {
    max-width: 100%;
    max-height: 360px;
    height: auto;
    border-radius: 6px;
    margin: 4px 0;
    display: block;
  }

  .rendered-content :global(.fc-audio) {
    width: 100%;
    max-width: 340px;
    height: 36px;
    margin: 4px 0;
    display: block;
  }

  .rendered-content :global(.fc-video) {
    max-width: 100%;
    max-height: 360px;
    border-radius: 6px;
    margin: 4px 0;
    display: block;
  }

  .rendered-content :global(ul),
  .rendered-content :global(ol) {
    margin: 0;
    padding-left: 0;
    list-style-position: inside;
  }

  .rendered-content :global(li) {
    margin: 0;
    line-height: inherit;
  }

  .rendered-content :global(li > p) {
    margin: 0;
    display: inline;
  }

  .rendered-content :global(hr) {
    border: none;
    border-top: 1px solid var(--border);
    margin: 8px 0;
  }

  .rendered-content :global(table) {
    border-collapse: collapse;
    width: 100%;
    margin: 4px 0;
  }

  .rendered-content :global(th),
  .rendered-content :global(td) {
    border: 1px solid var(--border);
    padding: 6px 10px;
    text-align: left;
  }

  .rendered-content :global(th) {
    background: var(--bg-secondary);
    font-weight: 600;
  }

  .rendered-content :global(img) {
    max-width: 100%;
    border-radius: 6px;
  }

  /* Query block styles */
  .query-block {
    width: 100%;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
    font-size: 13px;
    background: var(--bg-secondary, var(--bg-sidebar));
  }

  .query-header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    background: var(--bg-input, rgba(0,0,0,0.1));
    border-bottom: 1px solid var(--border);
    color: var(--text-muted);
    font-size: 12px;
  }

  .query-edit-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    padding: 2px 4px;
    border-radius: 3px;
    display: flex;
    align-items: center;
  }

  .query-edit-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .query-label {
    flex: 1;
    font-size: 11px;
    color: var(--text-muted);
    opacity: 0.7;
  }

  .query-refresh {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 14px;
    padding: 0 4px;
    border-radius: 3px;
    line-height: 1;
  }

  .query-refresh:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .query-loading,
  .query-empty,
  .query-error {
    padding: 10px 12px;
    color: var(--text-muted);
    font-size: 12px;
  }

  .query-error {
    color: #e57373;
  }

  .query-table-wrap {
    overflow-x: auto;
    max-width: 100%;
  }

  .query-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
    table-layout: auto;
  }

  .query-table th {
    position: sticky;
    top: 0;
    background: var(--bg-input, rgba(0,0,0,0.15));
    font-weight: 600;
    text-align: left;
    padding: 5px 10px;
    border-bottom: 1px solid var(--border);
    color: var(--text-muted);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.3px;
    white-space: nowrap;
  }

  .query-table td {
    padding: 5px 10px;
    border-bottom: 1px solid var(--border);
    color: var(--text-secondary);
    word-wrap: break-word;
    overflow-wrap: break-word;
    white-space: normal;
    max-width: 350px;
  }

  .query-table td:has(.query-cell-content) {
    white-space: normal;
    max-width: 450px;
  }

  .query-table tbody tr {
    cursor: pointer;
  }

  /* CodeMirror autocomplete dropdown theme override */
  :global(.cm-tooltip-autocomplete) {
    background: var(--bg-sidebar) !important;
    border: 1px solid var(--border) !important;
    border-radius: 6px !important;
    box-shadow: 0 4px 16px rgba(0,0,0,0.25) !important;
  }

  :global(.cm-tooltip-autocomplete ul li) {
    color: var(--text-secondary) !important;
    font-size: 13px !important;
    padding: 5px 10px !important;
  }

  :global(.cm-tooltip-autocomplete ul li[aria-selected]) {
    background: var(--bg-active) !important;
    color: var(--text-primary) !important;
  }

  :global(.cm-completionDetail) {
    color: var(--text-muted) !important;
    font-size: 11px !important;
  }
</style>
