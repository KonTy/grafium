<script lang="ts">
  import { onMount, tick } from "svelte";
  import { EditorView, keymap, placeholder as cmPlaceholder, lineNumbers, tooltips } from "@codemirror/view";
  import { EditorState, EditorSelection, Prec, Transaction } from "@codemirror/state";
  import { defaultKeymap, indentWithTab, history, historyKeymap, undo, redo } from "@codemirror/commands";
  import { autocompletion, closeCompletion, startCompletion, completionStatus, type CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
  import { markdown } from "@codemirror/lang-markdown";
  import { save as saveDialog } from "@tauri-apps/plugin-dialog";
  import { open as openExternal } from "@tauri-apps/plugin-shell";
  import {
    clearMarkdownImageWidth,
    renderBlock,
    hydrateAssetMedia,
    assetBaseDirFor,
    markdownHeadingSlug,
    setMarkdownImageWidth,
  } from "../lib/markdown";
  import {
    updateBlock,
    createBlock,
    deleteBlock,
    runQuery,
    cycleTaskState,
    updateTaskState,
    getBlock,
    getBlockPageTitle,
    setTaskDate,
    downloadAsset,
    readAssetDataUrl,
    resolveAssetFilePath,
    saveImageToPath,
    searchPageTitles,
    listPages,
  } from "../lib/api";
  import type { QueryRow } from "../lib/api";
  import type { BlockContentChange } from "../lib/undoStack";
  import { keymap_manager } from "../lib/keymap";
  import { htmlToMarkdown, splitMarkdownIntoBlocks, localizeImages } from "../lib/htmlToMd";
  import { buildSaveContext, persistBlockContentIfChanged } from "../lib/persistence";
  import { EDITOR_UNDO_MIN_DEPTH } from "../lib/editorUndo";
  import { telemetry } from "../lib/telemetry";
  import type { PasteBlock } from "../lib/htmlToMd";
  import type { Block } from "../lib/api";
  import { FORMATTING_SLASH_COMMANDS, angleTemplateMenu } from "../lib/slashCommands";
  import {
    loadWikiLinkPages,
    wikiLinkCloseExtra,
    wikiLinkReplacement,
    wikiLinkToken,
  } from "../lib/wikiLinkCompletion";
  import { toggleWrapText, wrapPageLinkText } from "../lib/editorFormat";
  import { insertAtCursor } from "../lib/editorInsert";
  import { contextMenuPositionFromEvent } from "../lib/contextMenu";
  import {
    assetPathFromImageUrl,
    formatImageScale,
    formatScaledImageDimensions,
    imageIndexFromElement,
    IMAGE_SIZE_SCALES,
    imageSourceUrl,
    renderedImageBaseSize,
    scaledImageDimensions,
  } from "../lib/imageSizing";
  import { bulletToTodoContent, isTaskContent, normalizeTaskPrefix, splitImeEnterContent } from "../lib/taskSyntax";
  import { isFencedCodeBlock } from "../lib/codeFence";
  import { sortMarkdownTableColumn, type TableSortDirection } from "../lib/markdownTableSort";
  import DatePicker from "./DatePicker.svelte";
  import MobileEditorBar from "./MobileEditorBar.svelte";

  interface Props {
    block: Block;
    pageId: string;
    pageTitle?: string;
    /** Graph-relative directory of the page's markdown file, so a page-relative
     * asset reference (`assets/x.png`) resolves beside the page. */
    assetBaseDir?: string;
    bookMode?: boolean;
    depth?: number;
    /// Per-ancestor-level flags for drawing the vertical "thread" guide line
    /// (see `getAncestorGuides` in pageContentVirtualization.ts). Index i
    /// corresponds to indent level i; true draws a full-height line at that
    /// level's column, false/undefined draws nothing (that ancestor has no
    /// more siblings below, so there's nothing to visually connect to).
    guides?: boolean[];
    /// Colored L into this bullet when it sits on the focused path.
    threadElbow?: boolean;
    /// Column continued through a preceding sibling and all its descendants.
    threadContinuationDepth?: number | null;
    /// Colored stem from an ancestor into its children (never the focused row).
    threadStem?: boolean;
    /// When false, skip all indent/thread decorations (Settings toggle).
    showGuides?: boolean;
    focused?: boolean;
    selected?: boolean;
    hasChildren?: boolean;
    collapsed?: boolean;
    onFocus?: (blockId: string) => void;
    onBlur?: (blockId: string) => void;
    onEnter?: (blockId: string, content: string, orderIndex: number, atStart: boolean, remainder?: string) => void;
    onDelete?: (blockId: string) => void;
    onIndent?: (blockId: string, direction: "in" | "out", currentContent?: string) => void;
    onNavigate?: (blockId: string, direction: "up" | "down", caretX?: number) => void;
    onAnchor?: (blockId: string) => void;
    onBulletClick?: (blockId: string, event: MouseEvent) => void;
    onPasteBlocks?: (
      blockId: string,
      blocks: PasteBlock[],
      anchorEdit?: { beforeContent: string; afterContent: string },
      persistAnchor?: () => Promise<void>,
    ) => void | Promise<void>;
    onToggleCollapse?: (blockId: string) => void;
    onContentChange?: (pageId: string, change: BlockContentChange) => void;
  }

  let {
    block,
    pageId,
    pageTitle = "",
    assetBaseDir = "",
    bookMode = false,
    depth = 0,
    guides = [],
    threadElbow = false,
    threadContinuationDepth = null,
    threadStem = false,
    showGuides = true,
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
    onAnchor,
    onBulletClick,
    onPasteBlocks,
    onToggleCollapse,
    onContentChange,
  }: Props = $props();

  let editorContainer: HTMLDivElement;
  let editorView: EditorView | undefined;
  let savedState: EditorState | undefined;
  let blurTeardownTimer: number | undefined;
  let shiftHeld = false;
  let isEditing = $state(false);
  /// Set when the user dismisses the `[[` picker with Escape so the
  /// update listener does not immediately reopen it while the token remains.
  let wikiCompletionDismissed = false;
  let isCodeBlock = $derived(detectCodeBlock(block.content));
  let isFenceBlock = $derived(isFencedCodeBlock(block.content));
  let renderedHtml = $derived(renderBlock(block.content, assetBaseDir));
  let isTableBlock = $derived(renderedHtml.includes("<table"));

  /**
   * Base directory for one query result row.
   *
   * Query results are rows from arbitrary pages, so this block's own directory
   * is the wrong answer for them — and worse than no answer, since a
   * same-named file next to the query would render in place of the real one.
   * The row's own `file_path` is used when the query happened to select it,
   * and otherwise the graph root, which is where the historical `../assets/`
   * form resolves anyway.
   */
  function queryRowBaseDir(row: [string, unknown][]): string {
    const filePath = row.find(([col]) => col === "file_path")?.[1];
    return typeof filePath === "string" ? assetBaseDirFor(filePath) : "";
  }
  let saveError = $state<string | null>(null);
  let imageMenuMessage = $state<string | null>(null);
  let imageMenuMessageTimer: number | undefined;
  let pendingSaveContent = $state<string | null>(null);
  let nextPasteId = 0;
  let pendingPastes = $state<{ id: number; markdown: string; error: string | null }[]>([]);
  let finishEditingPromise: Promise<boolean> | null = null;
  type ImageSizeMenu = {
    index: number;
    x: number;
    y: number;
    baseWidth: number;
    baseHeight: number;
    sourceUrl: string;
    markdownSrc: string;
    assetPath: string | null;
    altText: string;
  };
  type TextLinkRange = {
    from: number;
    to: number;
    text: string;
  };
  type MakeLinkMenu = {
    x: number;
    y: number;
    from: number;
    to: number;
    text: string;
  };
  const EDITOR_MAKE_LINK_CACHE_MS = 30_000;
  let imageSizeMenu: ImageSizeMenu | null = $state(null);
  let makeLinkMenu: MakeLinkMenu | null = $state(null);
  let lastEditorPageLinkRange: (TextLinkRange & { capturedAt: number }) | null = null;
  let blockContentEl = $state<HTMLElement | null>(null);

  // Rendered-content container, used to hydrate <audio>/<video> media that
  // WebKitGTK can't load from the custom asset scheme.
  let renderedEl = $state<HTMLElement | null>(null);
  let tableSort: { tableIndex: number; columnIndex: number; direction: TableSortDirection } | null = $state(null);
  $effect(() => {
    void renderedHtml;
    const el = renderedEl;
    if (!el) return;
    let cancelled = false;
    let cleanup: (() => void) | undefined;
    queueMicrotask(() => {
      if (cancelled) return;
      cleanup = hydrateAssetMedia(el);
    });

    $effect(() => {
      if (!imageSizeMenu) return;
      const closeMenu = (event: MouseEvent | PointerEvent) => {
        const target = event.target as Element | null;
        if (target?.closest?.(".image-size-menu")) return;
        imageSizeMenu = null;
      };
      const handleKeydown = (event: KeyboardEvent) => {
        if (event.key === "Escape") {
          imageSizeMenu = null;
        }
      };
      window.addEventListener("pointerdown", closeMenu);
      window.addEventListener("contextmenu", closeMenu);
      window.addEventListener("keydown", handleKeydown);
      return () => {
        window.removeEventListener("pointerdown", closeMenu);
        window.removeEventListener("contextmenu", closeMenu);
        window.removeEventListener("keydown", handleKeydown);
      };
    });
    return () => {
      cancelled = true;
      cleanup?.();
    };
  });

  $effect(() => {
    void renderedHtml;
    const el = renderedEl;
    const sort = tableSort;
    if (!el || !sort) return;
    queueMicrotask(() => {
      const table = el.querySelectorAll("table")[sort.tableIndex];
      if (!table) return;
      table.querySelectorAll("th").forEach((th, i) => {
        th.setAttribute(
          "aria-sort",
          i === sort.columnIndex ? (sort.direction === "asc" ? "ascending" : "descending") : "none",
        );
      });
    });
  });

  $effect(() => {
    if (!makeLinkMenu) return;
    const closeMenu = (event: MouseEvent | PointerEvent) => {
      const target = event.target as Element | null;
      if (target?.closest?.(".make-link-menu")) return;
      makeLinkMenu = null;
    };
    const handleKeydown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        makeLinkMenu = null;
      }
    };
    window.addEventListener("pointerdown", closeMenu);
    window.addEventListener("contextmenu", closeMenu);
    window.addEventListener("keydown", handleKeydown);
    return () => {
      window.removeEventListener("pointerdown", closeMenu);
      window.removeEventListener("contextmenu", closeMenu);
      window.removeEventListener("keydown", handleKeydown);
    };
  });

  $effect(() => {
    const handleReplacement = (event: Event) => {
      const detail = (event as CustomEvent<{ pageId: string; blockId: string; content: string }>).detail;
      if (!detail || detail.pageId !== pageId || detail.blockId !== block.id) return;
      block.content = detail.content;
      const view = editorView;
      if (!view) return;
      const current = view.state.doc.toString();
      if (current === detail.content) return;
      view.dispatch({
        changes: { from: 0, to: current.length, insert: detail.content },
        annotations: Transaction.addToHistory.of(false),
      });
    };
    window.addEventListener("grafium-block-content-replaced", handleReplacement);
    return () => window.removeEventListener("grafium-block-content-replaced", handleReplacement);
  });

  $effect(() => {
    const shell = blockContentEl?.closest(".block-shell");
    if (!shell) return;
    if (!imageSizeMenu) {
      shell.classList.remove("image-menu-shell");
      return;
    }
    shell.classList.add("image-menu-shell");
    return () => {
      shell.classList.remove("image-menu-shell");
    };
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
  // Query results are unbounded — a broad query can return thousands of rows,
  // each of which renders markdown per cell. Render a page at a time so a
  // large result set cannot lock up the editor.
  const QUERY_ROW_PAGE = 100;
  let queryRowLimit = $state(QUERY_ROW_PAGE);
  let visibleQueryRows = $derived.by(() => {
    const rows = queryRows;
    return rows === null ? null : rows.slice(0, queryRowLimit);
  });
  let bulletMinHeight = $derived(getBulletMinHeight(block.content));
  let editorStyleClass = $derived(getEditorStyleClass(block.content));
  let isQuoteBlock = $derived(block.content.trimStart().startsWith(">"));
  let isVisuallyEmpty = $derived(isVisuallyEmptyBlock(block.content));
  let suppressBullet = $derived(
    getHeadingLevel(block.content) > 0 || isTaskContent(block.content) || isTableBlock || isFenceBlock
  );
  let showBlockMarker = $derived(
    !bookMode &&
      !isFenceBlock &&
      !queryExpression &&
      !isVisuallyEmpty &&
      !isQuoteBlock &&
      (hasChildren || !suppressBullet)
  );

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
      queryRowLimit = QUERY_ROW_PAGE;
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
      label: "/A",
      detail: "Set priority A (highest)",
      apply: "[#A] ",
    },
    {
      label: "/Priority B",
      detail: "Set priority B (medium)",
      apply: "[#B] ",
    },
    {
      label: "/B",
      detail: "Set priority B (medium)",
      apply: "[#B] ",
    },
    {
      label: "/Priority C",
      detail: "Set priority C (low)",
      apply: "[#C] ",
    },
    {
      label: "/C",
      detail: "Set priority C (low)",
      apply: "[#C] ",
    },
    // Formatting inserters (quote, headings, code) and callout admonitions.
    // Sorted after the task/priority entries so TODO/DONE muscle-memory is
    // unaffected. These are pure text insertions with an explicit cursor
    // offset (e.g. callouts drop the cursor on the blank body line).
    ...FORMATTING_SLASH_COMMANDS,
  ];

  // Toggle markdown emphasis markers (`*`, `**`, `~~`) around the current
  // selection via the pure toggleWrapText helper, then replace the doc.
  function applyToggleWrap(view: EditorView, marker: string): boolean {
    const { from, to } = view.state.selection.main;
    const r = toggleWrapText(view.state.doc.toString(), from, to, marker);
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: r.doc },
      selection:
        r.selStart === r.selEnd
          ? EditorSelection.cursor(r.selStart)
          : EditorSelection.range(r.selStart, r.selEnd),
    });
    return true;
  }

  function slashCompletionSource(context: CompletionContext): CompletionResult | null {
    // Match a `/` optionally followed by word chars at the current position
    const match = context.matchBefore(/\/[^\s]*/);
    if (!match) return null;
    const typed = match.text.toLowerCase();
    const commands = typed === "/"
      ? SLASH_COMMANDS
      : SLASH_COMMANDS.filter((cmd) => cmd.label.toLowerCase().startsWith(typed));
    if (commands.length === 0) return null;

    return {
      from: match.from,
      filter: false,
      options: commands.map((cmd) => ({
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

  // `<`-triggered template menu: a Logseq-style path to the same callout
  // inserters (`< tip`, `< note`, …). Pure text insertion with a cursor offset.
  // Guarded so it only opens at a block/line start or after whitespace, and
  // only while the typed text is a prefix of a callout kind — otherwise a
  // comparison like `2 < 3` or an inline `<foo>` would hijack Enter/Escape.
  function angleCompletionSource(context: CompletionContext): CompletionResult | null {
    const head = context.state.selection.main.head;
    const line = context.state.doc.lineAt(head);
    const beforeCursor = line.text.slice(0, head - line.from);
    const menu = angleTemplateMenu(beforeCursor);
    if (!menu) return null;

    return {
      from: line.from + menu.from,
      filter: false,
      options: menu.options.map((cmd) => ({
        label: cmd.label,
        detail: cmd.detail,
        apply: (view: EditorView, _completion: unknown, from: number, to: number) => {
          view.dispatch({
            changes: { from, to, insert: cmd.apply },
            selection: EditorSelection.cursor(
              from + (cmd.cursorOffset ?? cmd.apply.length)
            ),
          });
        },
      })),
    };
  }

  async function wikiLinkCompletionSource(context: CompletionContext): Promise<CompletionResult | null> {
    const head = context.state.selection.main.head;
    const line = context.state.doc.lineAt(head);
    const beforeCursor = line.text.slice(0, head - line.from);
    const token = wikiLinkToken(beforeCursor);
    if (!token) return null;
    if (editorView && isInsideCodeFence(editorView)) return null;

    try {
      const pages = await loadWikiLinkPages(token.query, {
        search: searchPageTitles,
        listRecent: (limit) => listPages(limit, 0),
      });
      if (pages.length === 0) return null;
      const extra = wikiLinkCloseExtra(line.text.slice(head - line.from));
      return {
        from: line.from + token.from,
        to: head + extra,
        filter: false,
        options: pages.map((page) => ({
          label: page.title,
          detail: page.is_journal ? "journal" : "page",
          apply: wikiLinkReplacement(page.title),
        })),
      };
    } catch {
      return null;
    }
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

  function isVisuallyEmptyBlock(content: string): boolean {
    const normalized = content.replace(/[\u200B-\u200D\uFEFF]/g, "").trim();
    return normalized === "" || /^[-*+]$/.test(normalized);
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

  let enterHandling = false;

  function submitBlockEnter(view: EditorView): boolean {
    if (completionOpen(view)) return false;
    if (isInsideCodeFence(view)) {
      const { from } = view.state.selection.main;
      view.dispatch({
        changes: { from, to: from, insert: "\n" },
        selection: EditorSelection.cursor(from + 1),
      });
      return true;
    }
    const sel = view.state.selection.main;
    const atStart = sel.from === 0 && sel.to === 0;
    const { head, remainder } = splitImeEnterContent(view.state.doc.toString());
    if (head !== view.state.doc.toString()) {
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: head },
      });
    }
    onEnter?.(block.id, head, block.order_index, atStart && remainder === "", remainder);
    return true;
  }

  function submitBlockEnterOnce(view: EditorView): boolean {
    if (enterHandling) return true;
    enterHandling = true;
    try {
      return submitBlockEnter(view);
    } finally {
      queueMicrotask(() => {
        enterHandling = false;
      });
    }
  }

  // Save content on blur
  async function saveContent(content: string) {
    content = normalizeTaskPrefix(content);
    const context = buildSaveContext(block.id, pageId, block.content, content);
    telemetry("savecontext", () => (context));
    const beforeContent = block.content;
    try {
      const changed = await persistBlockContentIfChanged(block, content, (id, value) => updateBlock(id, value));
      saveError = null;
      pendingSaveContent = null;
      if (changed) {
        onContentChange?.(pageId, {
          blockId: block.id,
          beforeContent,
          afterContent: content,
        });
        telemetry("saveContent", () => (context));
      }
      return changed;
    } catch (e) {
      pendingSaveContent = content;
      saveError = `Failed to save changes: ${e instanceof Error ? e.message : String(e)}`;
      console.error("Failed to save block content:", e);
      throw e;
    }
  }

  function recordPersistedContentChange(
    changedPageId: string,
    blockId: string,
    beforeContent: string,
    afterContent: string,
  ) {
    if (beforeContent === afterContent) return;
    if (blockId === block.id) {
      block.content = afterContent;
    }
    onContentChange?.(changedPageId, {
      blockId,
      beforeContent,
      afterContent,
    });
  }

  function imageFilename(menu: ImageSizeMenu): string {
    const source = menu.assetPath || menu.markdownSrc || menu.sourceUrl;
    try {
      const url = new URL(source);
      const name = decodeURIComponent(url.pathname.split("/").pop() || "");
      return name || "image";
    } catch {
      const clean = source.split(/[?#]/)[0].replace(/\\/g, "/");
      return clean.split("/").pop() || "image";
    }
  }

  function markdownImageText(menu: ImageSizeMenu): string {
    const alt = menu.altText.replace(/]/g, "\\]");
    return `![${alt}](${menu.markdownSrc || menu.sourceUrl})`;
  }

  function openImageSizeMenu(event: MouseEvent, img: HTMLImageElement) {
    const index = imageIndexFromElement(img);
    if (index === null) return;
    const pos = contextMenuPositionFromEvent(event, { width: 236, height: 560 });
    const baseSize = renderedImageBaseSize(img, renderedEl?.clientWidth);
    const sourceUrl = imageSourceUrl(img);
    const assetPath = assetPathFromImageUrl(sourceUrl) ?? assetPathFromImageUrl(img.dataset.src);
    imageSizeMenu = {
      index,
      x: pos.x,
      y: pos.y,
      baseWidth: baseSize.width,
      baseHeight: baseSize.height,
      sourceUrl,
      markdownSrc: img.dataset.markdownSrc || assetPath || sourceUrl,
      assetPath,
      altText: img.alt || "",
    };
  }

  function selectedPageLinkRange(view: EditorView): TextLinkRange | null {
    const selection = view.state.selection.main;
    if (selection.empty) return null;
    const selected = view.state.doc.sliceString(selection.from, selection.to);
    if (!selected.trim() || selected.includes("\n")) return null;
    return { from: selection.from, to: selection.to, text: selected };
  }

  function rememberEditorPageLinkRange(view: EditorView): TextLinkRange | null {
    const range = selectedPageLinkRange(view);
    if (range) {
      lastEditorPageLinkRange = { ...range, capturedAt: Date.now() };
    }
    return range;
  }

  function cachedEditorPageLinkRange(view: EditorView): TextLinkRange | null {
    const range = lastEditorPageLinkRange;
    if (!range || Date.now() - range.capturedAt > EDITOR_MAKE_LINK_CACHE_MS) {
      lastEditorPageLinkRange = null;
      return null;
    }

    const doc = view.state.doc.toString();
    if (doc.slice(range.from, range.to) === range.text) {
      const { capturedAt: _capturedAt, ...cached } = range;
      return cached;
    }
    const fallback = doc.indexOf(range.text);
    if (fallback < 0) {
      lastEditorPageLinkRange = null;
      return null;
    }
    return { from: fallback, to: fallback + range.text.length, text: range.text };
  }

  function countOccurrences(text: string, needle: string): number {
    if (!needle) return 0;
    let count = 0;
    let index = text.indexOf(needle);
    while (index !== -1) {
      count += 1;
      index = text.indexOf(needle, index + needle.length);
    }
    return count;
  }

  function nthIndexOf(text: string, needle: string, occurrence: number): number {
    let remaining = Math.max(0, occurrence);
    let index = text.indexOf(needle);
    while (index !== -1 && remaining > 0) {
      remaining -= 1;
      index = text.indexOf(needle, index + needle.length);
    }
    return index;
  }

  function renderedSelectionPrefix(range: Range): string {
    if (!renderedEl) return "";
    const prefixRange = document.createRange();
    prefixRange.selectNodeContents(renderedEl);
    prefixRange.setEnd(range.startContainer, range.startOffset);
    const prefix = prefixRange.toString();
    prefixRange.detach();
    return prefix.replace(/\u00a0/g, " ");
  }

  function selectedRenderedPageLinkRange(): TextLinkRange | null {
    const selection = window.getSelection();
    if (!selection || selection.isCollapsed || selection.rangeCount === 0 || !renderedEl) return null;

    const range = selection.getRangeAt(0);
    if (!renderedEl.contains(range.startContainer) || !renderedEl.contains(range.endContainer)) {
      return null;
    }

    const selected = selection.toString().replace(/\u00a0/g, " ").trim();
    if (!selected || selected.includes("\n")) return null;

    const occurrence = countOccurrences(renderedSelectionPrefix(range), selected);
    const from = nthIndexOf(block.content, selected, occurrence);
    if (from < 0) return null;

    return { from, to: from + selected.length, text: selected };
  }

  function openMakeLinkMenuForRange(event: MouseEvent, range: TextLinkRange | null) {
    if (!range) return false;
    const pos = contextMenuPositionFromEvent(event, { width: 160, height: 48 });
    imageSizeMenu = null;
    makeLinkMenu = {
      x: pos.x,
      y: pos.y,
      ...range,
    };
    return true;
  }

  function openEditorMakeLinkMenu(event: MouseEvent, view: EditorView) {
    return openMakeLinkMenuForRange(event, rememberEditorPageLinkRange(view) ?? cachedEditorPageLinkRange(view));
  }

  function openRenderedMakeLinkMenu(event: MouseEvent) {
    return openMakeLinkMenuForRange(event, selectedRenderedPageLinkRange());
  }

  async function makeSelectedTextLink() {
    const menu = makeLinkMenu;
    const view = editorView;
    if (!menu) return;

    const doc = view?.state.doc.toString() ?? block.content;
    let from = Math.max(0, Math.min(menu.from, doc.length));
    let to = Math.max(from, Math.min(menu.to, doc.length));
    if (doc.slice(from, to) !== menu.text) {
      const fallback = doc.indexOf(menu.text);
      if (fallback < 0) {
        makeLinkMenu = null;
        return;
      }
      from = fallback;
      to = fallback + menu.text.length;
    }
    const result = wrapPageLinkText(doc, from, to);
    if (view) {
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: result.doc },
        selection:
          result.selStart === result.selEnd
            ? EditorSelection.cursor(result.selStart)
            : EditorSelection.range(result.selStart, result.selEnd),
      });
    } else {
      try {
        await saveContent(result.doc);
      } catch {
        // saveContent already surfaces the error state.
      }
    }
    makeLinkMenu = null;
    lastEditorPageLinkRange = null;
    window.getSelection()?.removeAllRanges();
    view?.focus();
  }

  async function withImageMenuAction(label: string, action: (menu: ImageSizeMenu) => Promise<void>) {
    const menu = imageSizeMenu;
    if (!menu) return;
    try {
      await action(menu);
      imageSizeMenu = null;
      saveError = null;
      showImageMenuMessage(`${label[0].toUpperCase()}${label.slice(1)} complete`);
    } catch (e) {
      saveError = `Failed to ${label}: ${e instanceof Error ? e.message : String(e)}`;
      console.error(`Failed to ${label}:`, e);
    }
  }

  function showImageMenuMessage(message: string) {
    imageMenuMessage = message;
    if (imageMenuMessageTimer) clearTimeout(imageMenuMessageTimer);
    imageMenuMessageTimer = window.setTimeout(() => {
      imageMenuMessage = null;
      imageMenuMessageTimer = undefined;
    }, 1800);
  }

  async function writeClipboardText(text: string) {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(text);
        return;
      }
    } catch {
      // Fall back below; WebKitGTK can reject async clipboard despite a click.
    }

    const scratch = document.createElement("textarea");
    scratch.value = text;
    scratch.style.position = "fixed";
    scratch.style.left = "-9999px";
    scratch.style.opacity = "0";
    document.body.appendChild(scratch);
    scratch.select();
    const copied = document.execCommand("copy");
    scratch.remove();
    if (!copied) {
      throw new Error("clipboard text API is unavailable");
    }
  }

  async function copyImageAddress() {
    await withImageMenuAction("copy image address", async (menu) => {
      await writeClipboardText(menu.markdownSrc || menu.sourceUrl);
    });
  }

  async function copyMarkdownImage() {
    await withImageMenuAction("copy Markdown image", async (menu) => {
      await writeClipboardText(markdownImageText(menu));
    });
  }

  async function copyImage() {
    await withImageMenuAction("copy image", async (menu) => {
      if (!navigator.clipboard?.write || typeof ClipboardItem === "undefined") {
        await writeClipboardText(menu.markdownSrc || menu.sourceUrl);
        return;
      }
      const source = menu.assetPath ? await readAssetDataUrl(menu.assetPath) : menu.sourceUrl;
      if (!source) throw new Error("image source is unavailable");
      const response = await fetch(source);
      if (!response.ok) throw new Error(`could not read image (${response.status})`);
      const blob = await response.blob();
      if (!blob.type.startsWith("image/")) {
        throw new Error(`unsupported image type ${blob.type || "unknown"}`);
      }
      await navigator.clipboard.write([new ClipboardItem({ [blob.type]: blob })]);
    });
  }

  async function openImageExternally() {
    await withImageMenuAction("open image", async (menu) => {
      if (menu.assetPath) {
        await openExternal(await resolveAssetFilePath(menu.assetPath));
        return;
      }
      if (!menu.sourceUrl) throw new Error("image source is unavailable");
      await openExternal(menu.sourceUrl);
    });
  }

  async function saveImageAs() {
    await withImageMenuAction("save image", async (menu) => {
      const destination = await saveDialog({
        title: "Save Image As",
        defaultPath: imageFilename(menu),
        filters: [
          { name: "Images", extensions: ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif"] },
        ],
      });
      if (!destination) return;
      const source = menu.assetPath || menu.sourceUrl;
      if (!source) throw new Error("image source is unavailable");
      await saveImageToPath(source, destination);
    });
  }

  async function commitImageWidth(index: number, width: number) {
    const nextContent = setMarkdownImageWidth(block.content, index, width);
    if (nextContent === block.content) return;
    await saveContent(nextContent);
  }

  async function clearImageWidth(index: number) {
    const nextContent = clearMarkdownImageWidth(block.content, index);
    if (nextContent === block.content) return;
    await saveContent(nextContent);
  }

  function imageScaleSizeLabel(scale: number): string {
    const baseWidth = imageSizeMenu?.baseWidth ?? 240;
    const baseHeight = imageSizeMenu?.baseHeight ?? 180;
    return formatScaledImageDimensions(scale, baseWidth, baseHeight);
  }

  async function chooseImageScale(scale: number) {
    const menu = imageSizeMenu;
    if (!menu) return;
    try {
      const next = scaledImageDimensions(scale, menu.baseWidth, menu.baseHeight);
      await commitImageWidth(menu.index, next.width);
      imageSizeMenu = null;
    } catch {
      // saveContent already surfaced the error on the block.
    }
  }

  async function resetImageScale() {
    const menu = imageSizeMenu;
    if (!menu) return;
    try {
      await clearImageWidth(menu.index);
      imageSizeMenu = null;
    } catch {
      // saveContent already surfaced the error on the block.
    }
  }

  function teardownEditor(view: EditorView, notifyBlur: boolean) {
    savedState = view.state;
    if ((window as any).__activeEditorView === view) {
      (window as any).__activeEditorView = undefined;
    }
    view.destroy();
    if (editorView === view) {
      editorView = undefined;
    }
    isEditing = false;
    makeLinkMenu = null;
    keymap_manager.isEditing = false;
    if (notifyBlur) {
      onBlur?.(block.id);
    }
  }

  async function closeEditorAfterSave(view: EditorView, notifyBlur: boolean): Promise<boolean> {
    if (finishEditingPromise) {
      return finishEditingPromise;
    }

    finishEditingPromise = (async () => {
      const content = view.state.doc.toString();
      try {
        await saveContent(content);
      } catch {
        if (editorView === view) {
          queueMicrotask(() => view.focus());
        }
        return false;
      }

      if (editorView === view) {
        teardownEditor(view, notifyBlur);
      }
      return true;
    })();

    try {
      return await finishEditingPromise;
    } finally {
      finishEditingPromise = null;
    }
  }

  async function retrySave() {
    const content = editorView?.state.doc.toString() ?? pendingSaveContent;
    if (content == null) return;
    try {
      await saveContent(content);
    } catch {
      // saveContent already surfaces the error state
    }
  }

  // Detect if cursor is inside a code fence (``` ... ```)
  function completionOpen(view: EditorView): boolean {
    const status = completionStatus(view.state);
    return status === "active" || status === "pending";
  }

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
  let pendingEndCaret = false;
  let pendingInsert: string | null = null;

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

  /** Open this block for editing and put the caret at the end of its text. */
  export function focusAtEnd() {
    pendingEndCaret = true;
    if (isEditing && editorView) {
      placeEndCaret(editorView);
    } else {
      startEditing();
    }
  }

  /** Insert text at the caret. If the block is not being edited, open it at the end first. */
  export function insertText(text: string) {
    pendingInsert = text;
    if (isEditing && editorView) {
      flushPendingInsert(editorView);
    } else {
      pendingEndCaret = true;
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

  function placeEndCaret(view: EditorView) {
    if (!pendingEndCaret) return;
    pendingEndCaret = false;
    view.dispatch({ selection: EditorSelection.cursor(view.state.doc.length) });
    view.focus();
  }

  function flushPendingInsert(view: EditorView) {
    if (!pendingInsert) return;
    const text = pendingInsert;
    pendingInsert = null;
    insertAtCursor(view, text);
  }

  function startEditing() {
    if (isEditing) return;
    imageSizeMenu = null;
    makeLinkMenu = null;
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

  function clipboardMarkdown(data: DataTransfer | null): string | null {
    if (!data) return null;
    const markdown = data.getData("text/markdown").trim();
    if (markdown) return markdown;

    const html = data.getData("text/html");
    if (html.trim()) return htmlToMarkdown(html);

    const text = data.getData("text/plain").trim();
    return text || null;
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
          history({ minDepth: EDITOR_UNDO_MIN_DEPTH }),
          autocompletion({
            override: [slashCompletionSource, angleCompletionSource, wikiLinkCompletionSource],
            activateOnTyping: false,
            closeOnBlur: false,
          }),
          // Keep the completion menu out of the block stacking context so later
          // journal/page blocks cannot paint through it.
          tooltips({ parent: document.body }),
          Prec.highest(keymap.of([
            {
              key: "Escape",
              run: (view) => {
                if (completionOpen(view)) {
                  wikiCompletionDismissed = true;
                  closeCompletion(view);
                  return true;
                }
                void stopEditing();
                return true;
              },
            },
            {
              key: "Enter",
              run: (view) => submitBlockEnterOnce(view),
            },
          ])),
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
              run: (view) => submitBlockEnterOnce(view),
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
                if (completionOpen(view)) return false;
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
            // Selection-formatting shortcuts. Note: Ctrl+B and Ctrl+Shift+A are
            // deliberately NOT bound here — they belong to window-level sidebar
            // toggles.
            {
              key: "Mod-i",
              run: (view) => applyToggleWrap(view, "*"),
            },
            {
              key: "Mod-Shift-b",
              run: (view) => applyToggleWrap(view, "**"),
            },
            {
              key: "Mod-Shift-k",
              run: (view) => applyToggleWrap(view, "~~"),
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
            if (update.docChanged && !isInsideCodeFence(update.view)) {
              const prev = update.startState.doc.toString();
              const next = update.state.doc.toString();
              const pasted = update.transactions.some(
                (tr) => tr.isUserEvent("input.paste") || tr.isUserEvent("input.drop"),
              );
              if (!pasted && !shiftHeld && !prev.includes("\n") && next.includes("\n")) {
                submitBlockEnterOnce(update.view);
                return;
              }
            }
            if (!update.view.hasFocus) return;
            rememberEditorPageLinkRange(update.view);
            const sel = update.state.selection.main;
            if (!sel.empty) return;

            const line = update.state.doc.lineAt(sel.head);
            const beforeCursor = line.text.slice(0, sel.head - line.from);
            const slashToken = beforeCursor.match(/(?:^|\s)\/[^\s]*$/);
            // Open the callout template menu when a guarded `<` token is typed,
            // mirroring the slash trigger. `angleTemplateMenu` returns null for
            // mid-word `<`, comparisons, or non-matching text so ordinary typing
            // is never hijacked.
            const angleOpen = angleTemplateMenu(beforeCursor) !== null;
            const wikiOpen = wikiLinkToken(beforeCursor) !== null;
            if (!slashToken && !angleOpen && !wikiOpen) {
              wikiCompletionDismissed = false;
              return;
            }

            const prevStatus = completionStatus(update.startState);
            const status = completionStatus(update.state);
            // Wiki-link results come from the DB, so re-query as the title
            // fragment changes. Slash/`<` menus filter locally and only need
            // to open once. Escape dismisses the picker; keep it closed until
            // the user types again inside `[[`.
            if (wikiOpen && update.docChanged) {
              wikiCompletionDismissed = false;
              startCompletion(update.view);
              return;
            }
            if (
              wikiOpen &&
              !update.docChanged &&
              (prevStatus === "active" || prevStatus === "pending") &&
              status === null
            ) {
              wikiCompletionDismissed = true;
              return;
            }
            if (wikiOpen && wikiCompletionDismissed) return;
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
            beforeinput: (event, view) => {
              const inputType = (event as InputEvent).inputType;
              if (inputType !== "insertLineBreak" && inputType !== "insertParagraph") {
                return false;
              }
              if (shiftHeld || isInsideCodeFence(view)) return false;
              event.preventDefault();
              submitBlockEnterOnce(view);
              return true;
            },
            keydown: (event) => {
              if (event.key === "Shift") shiftHeld = true;
            },
            keyup: (event) => {
              if (event.key === "Shift") shiftHeld = false;
            },
            contextmenu: (event, view) => {
              if (!openEditorMakeLinkMenu(event, view)) return false;
              event.preventDefault();
              event.stopPropagation();
              return true;
            },
            paste: (event, view) => {
              const md = clipboardMarkdown(event.clipboardData);
              if (!md) return false;
              event.preventDefault();

              if (shiftHeld || !onPasteBlocks) {
                // Ctrl+Shift+V: paste everything into this one block
                const { from, to } = view.state.selection.main;
                view.dispatch({
                  changes: { from, to, insert: md },
                  selection: EditorSelection.cursor(from + md.length),
                });
                // Download images in background and update content
                void localizeImages(md, (u) => downloadAsset(u, pageId)).then(async (localized) => {
                  if (localized !== md) {
                    const doc = view.state.doc.toString();
                    const updated = doc.replace(md, localized);
                    view.dispatch({ changes: { from: 0, to: doc.length, insert: updated } });
                    try {
                      await saveContent(updated);
                    } catch {
                      // saveContent already surfaces the error state
                    }
                  }
                });
              } else {
                // Ctrl+V: split into separate blocks with hierarchy
                const chunks = splitMarkdownIntoBlocks(md);
                if (chunks.length === 0) return true;
                const beforePasteContent = view.state.doc.toString();
                // First chunk goes into the current block at cursor
                const { from, to } = view.state.selection.main;
                view.dispatch({
                  changes: { from, to, insert: chunks[0].content },
                  selection: EditorSelection.cursor(from + chunks[0].content.length),
                });
                // Remaining chunks become new blocks (with depth info)
                if (chunks.length > 1) {
                  const content = view.state.doc.toString();
                  const pasteId = nextPasteId++;
                  pendingPastes = [...pendingPastes, {
                    id: pasteId,
                    markdown: chunks.slice(1).map((chunk) => `${"  ".repeat(chunk.depth)}${chunk.content}`).join("\n\n"),
                    error: null,
                  }];
                  void (async () => {
                    try {
                      await onPasteBlocks?.(block.id, chunks.slice(1), {
                        beforeContent: beforePasteContent,
                        afterContent: content,
                      }, async () => { await saveContent(content); });
                      pendingPastes = pendingPastes.filter((paste) => paste.id !== pasteId);
                    } catch (error) {
                      pendingPastes = pendingPastes.map((paste) => paste.id === pasteId
                        ? { ...paste, error: error instanceof Error ? error.message : String(error) }
                        : paste);
                    }
                  })();
                }
                // Download images in all pasted blocks in background
                void localizeImages(md, (u) => downloadAsset(u, pageId)).then(async (localized) => {
                  if (localized !== md) {
                    // Re-split and update the first block
                    const localChunks = splitMarkdownIntoBlocks(localized);
                    if (localChunks[0]?.content !== chunks[0]?.content) {
                      const newContent = view.state.doc.toString().replace(chunks[0].content, localChunks[0].content);
                      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: newContent } });
                      try {
                        await saveContent(newContent);
                      } catch {
                        // saveContent already surfaces the error state
                      }
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
                blurTeardownTimer = undefined;
                void closeEditorAfterSave(view, true);
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
            // Android IMEs often insert "\n" instead of firing the Enter keymap.
            if (text.includes("\n") && !shiftHeld && !isInsideCodeFence(view)) {
              const nl = text.search(/\r?\n/);
              const before = nl >= 0 ? text.slice(0, nl) : "";
              if (before) {
                view.dispatch({
                  changes: { from, to, insert: before },
                  selection: EditorSelection.cursor(from + before.length),
                });
              }
              submitBlockEnterOnce(view);
              return true;
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
        const onEnterKey = (e: KeyboardEvent) => {
          if (e.key !== "Enter" || e.shiftKey || e.altKey || e.ctrlKey || e.metaKey) return;
          if (e.isComposing || e.keyCode === 229) return;
          if (!editorView || editorView !== view) return;
          if (completionOpen(view)) return;
          e.preventDefault();
          e.stopImmediatePropagation();
          submitBlockEnterOnce(view);
        };
        view.contentDOM.addEventListener("keydown", onEnterKey, true);
      }

      // Column-preserving vertical navigation: when this block was reached by
      // pressing Arrow Up/Down in an adjacent block, drop the caret at the same
      // viewport x on the appropriate (top/bottom) visual line — like MS Word.
      placeNavCaret(editorView);
      placeEndCaret(editorView);
      flushPendingInsert(editorView);
  }

  async function stopEditing() {
    if (blurTeardownTimer !== undefined) {
      window.clearTimeout(blurTeardownTimer);
      blurTeardownTimer = undefined;
    }

    if (!editorView) {
      isEditing = false;
      keymap_manager.isEditing = false;
      return true;
    }

    const didClose = await closeEditorAfterSave(editorView, false);
    if (!didClose) {
      return false;
    }

    // Ensure focus does not remain on a stale contenteditable node.
    requestAnimationFrame(() => {
      const active = document.activeElement as HTMLElement | null;
      if (!active) return;
      if (active.isContentEditable || active.closest(".cm-editor")) {
        active.blur();
      }
    });
    return true;
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

  function markCurrentBlock() {
    onAnchor?.(block.id);
  }

  async function handleTaskCycle() {
    try {
      const beforeContent = block.content;
      const newContent = await cycleTaskState(block.id);
      recordPersistedContentChange(pageId, block.id, beforeContent, newContent);
    } catch (e) {
      console.error("Failed to cycle task state:", e);
    }
  }

  function replaceEditorDoc(next: string, cursor = next.length) {
    const view = editorView;
    if (!view) return;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: next },
      selection: EditorSelection.cursor(Math.max(0, Math.min(cursor, next.length))),
    });
    view.focus();
  }

  async function handleMobileTodo() {
    const view = editorView;
    if (!view) return;
    const content = view.state.doc.toString();
    if (isTaskContent(content)) {
      try {
        const newContent = await cycleTaskState(block.id);
        recordPersistedContentChange(pageId, block.id, content, newContent);
        replaceEditorDoc(newContent);
      } catch (e) {
        console.error("Failed to cycle task state:", e);
      }
      return;
    }
    const next = bulletToTodoContent(content);
    replaceEditorDoc(next, content.trim() === "" ? next.length : next.length);
  }

  function handleMobileIndent(direction: "in" | "out") {
    onIndent?.(block.id, direction, editorView?.state.doc.toString());
  }

  function handleMobileLink() {
    const view = editorView;
    if (!view) return;
    const { from, to } = view.state.selection.main;
    const result = wrapPageLinkText(view.state.doc.toString(), from, to);
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: result.doc },
      selection:
        result.selStart === result.selEnd
          ? EditorSelection.cursor(result.selStart)
          : EditorSelection.range(result.selStart, result.selEnd),
    });
    view.focus();
  }

  function handleMobileInsert(text: string, complete = false) {
    const view = editorView;
    if (!view) return;
    insertAtCursor(view, text);
    if (complete) startCompletion(view);
  }

  async function handleTaskComplete() {
    try {
      const beforeContent = block.content;
      const newContent = await updateTaskState(block.id, "DONE");
      recordPersistedContentChange(pageId, block.id, beforeContent, newContent);
    } catch (e) {
      console.error("Failed to complete task:", e);
    }
  }

  function clickedTaskCheckbox(target: HTMLElement): HTMLElement | null {
    return target.closest(".task-checkbox") as HTMLElement | null;
  }

  async function handleQueryResultClick(e: MouseEvent) {
    const target = e.target as HTMLElement;

    const checkbox = clickedTaskCheckbox(target);
    if (checkbox) {
      e.stopPropagation();
      e.preventDefault();
      if (checkbox.dataset.taskAction !== "done" || queryBlockIdCol < 0) return;
      const row = checkbox.closest("tr");
      if (!row) return;
      const tbody = row.closest("tbody");
      if (!tbody) return;
      const rowIdx = Array.from(tbody.children).indexOf(row);
      if (rowIdx < 0 || !queryRows || rowIdx >= queryRows.length) return;
      const blockId = String(queryRows[rowIdx][queryBlockIdCol][1] ?? "");
      if (!blockId) return;
      try {
        const before = await getBlock(blockId);
        const newContent = await updateTaskState(blockId, "DONE");
        recordPersistedContentChange(before.page_id, blockId, before.content, newContent);
        if (queryExpression) {
          await runQueryBlock(queryExpression);
        }
      } catch (err) {
        console.error("Failed to complete task in query result:", err);
      }
      return;
    }

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
        const before = await getBlock(blockId);
        const newContent = await cycleTaskState(blockId);
        recordPersistedContentChange(before.page_id, blockId, before.content, newContent);
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

  function handleRenderedTableSort(e: MouseEvent, target: HTMLElement): boolean {
    const th = target.closest("th");
    const table = th?.closest("table");
    if (!th || !table || !renderedEl?.contains(table) || table.classList.contains("query-table")) {
      return false;
    }
    const headerRow = th.parentElement;
    if (!headerRow) return false;
    const columnIndex = Array.from(headerRow.children).indexOf(th);
    const tableIndex = Array.from(renderedEl.querySelectorAll("table")).indexOf(table);
    if (columnIndex < 0 || tableIndex < 0) return false;

    e.stopPropagation();
    e.preventDefault();
    const direction: TableSortDirection =
      tableSort?.tableIndex === tableIndex && tableSort.columnIndex === columnIndex && tableSort.direction === "asc"
        ? "desc"
        : "asc";
    const next = sortMarkdownTableColumn(block.content, tableIndex, columnIndex, direction);
    if (!next || next === block.content) {
      tableSort = { tableIndex, columnIndex, direction };
      return true;
    }
    tableSort = { tableIndex, columnIndex, direction };
    void saveContent(next);
    return true;
  }

  function handleRenderedClick(e: MouseEvent) {
    const target = e.target as HTMLElement;

    if (target instanceof HTMLImageElement && target.classList.contains("fc-img")) {
      e.stopPropagation();
      e.preventDefault();
      return;
    }

    const checkbox = clickedTaskCheckbox(target);
    if (checkbox) {
      e.stopPropagation();
      e.preventDefault();
      if (checkbox.dataset.taskAction === "done") {
        void handleTaskComplete();
      }
      return;
    }

    if (handleRenderedTableSort(e, target)) return;

    // Handle task marker clicks — cycle state
    if (target.classList.contains("task-marker")) {
      e.stopPropagation();
      e.preventDefault();
      handleTaskCycle();
      return;
    }

    const samePageAnchor = target.closest("a[href^='#']") as HTMLAnchorElement | null;
    if (samePageAnchor) {
      e.stopPropagation();
      e.preventDefault();
      const rawFragment = samePageAnchor.getAttribute("href")?.slice(1) ?? "";
      const fragment = (() => {
        try {
          return decodeURIComponent(rawFragment);
        } catch {
          return rawFragment;
        }
      })();
      if (fragment) {
        const fallbackFragment = bookChapterTextFragment(samePageAnchor);
        const fragments = fallbackFragment && fallbackFragment !== fragment
          ? [fragment, fallbackFragment]
          : [fragment];
        window.dispatchEvent(new CustomEvent("page-content-reveal-fragment", {
          detail: { pageId, fragment: fragments },
        }));
      }
      return;
    }

    const bookChapterAnchor = target.closest("a[href]") as HTMLAnchorElement | null;
    const bookChapterFragment = bookLocalChapterFragment(bookChapterAnchor);
    if (bookChapterFragment) {
      e.stopPropagation();
      e.preventDefault();
      window.dispatchEvent(new CustomEvent("page-content-reveal-fragment", {
        detail: { pageId, fragment: bookChapterFragment },
      }));
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

  function bookLocalChapterFragment(anchor: HTMLAnchorElement | null): string | null {
    if (!anchor || !pageTitle.startsWith("Books/")) return null;
    const href = anchor.getAttribute("href")?.trim() ?? "";
    if (!href || href.startsWith("#") || isExternalHref(href)) return null;
    if (!looksLikeBookLocalHref(href)) return null;

    return bookChapterTextFragment(anchor) ?? bookHrefFragment(href);
  }

  function bookChapterTextFragment(anchor: HTMLAnchorElement): string | null {
    if (!pageTitle.startsWith("Books/")) return null;
    const targetText = [cleanInlineText(anchor.textContent ?? "")]
      .find((candidate) => looksLikeChapterLinkText(candidate));
    return targetText ? markdownHeadingSlug(targetText) : null;
  }

  function bookHrefFragment(href: string): string | null {
    const targetText = [bookHrefLabel(href)].find((candidate) => looksLikeChapterLinkText(candidate));
    return targetText ? markdownHeadingSlug(targetText) : null;
  }

  function isExternalHref(href: string): boolean {
    return /^(?:https?:|mailto:|tel:|data:|blob:|grafium-asset:)/i.test(href.trim());
  }

  function looksLikeBookLocalHref(href: string): boolean {
    const cleaned = href.split(/[?#]/, 1)[0].trim().replace(/\\/g, "/");
    if (!cleaned || cleaned.startsWith("/")) return false;
    return /\.(?:x?html?|xml)$/i.test(cleaned) || cleaned.includes("/");
  }

  function bookHrefLabel(href: string): string {
    const cleaned = href.split(/[?#]/, 1)[0].trim().replace(/\\/g, "/");
    const leaf = cleaned.split("/").filter(Boolean).pop() ?? "";
    return leaf.replace(/\.(?:x?html?|xml)$/i, "").replace(/[_-]+/g, " ");
  }

  function cleanInlineText(text: string): string {
    return text.replace(/\s+/g, " ").trim();
  }

  function looksLikeChapterLinkText(text: string): boolean {
    const cleaned = cleanInlineText(text);
    const letterCount = Array.from(cleaned).filter((ch) => /\p{Letter}/u.test(ch)).length;
    return letterCount >= 3 && cleaned.length <= 140;
  }

  function handleRenderedContextMenu(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (!(target instanceof HTMLImageElement) || !target.classList.contains("fc-img")) {
      return;
    }
    e.stopPropagation();
    e.preventDefault();
    openImageSizeMenu(e, target);
  }

  async function handleDateSelect(date: string) {
    showDatePicker = false;
    try {
      const beforeContent = block.content;
      const newContent = await setTaskDate(block.id, datePickerKind, date || null);
      // Update the editor if open
      if (editorView) {
        editorView.dispatch({
          changes: { from: 0, to: editorView.state.doc.length, insert: newContent },
        });
      }
      recordPersistedContentChange(pageId, block.id, beforeContent, newContent);
    } catch (e) {
      console.error("Failed to set task date:", e);
    }
  }

  function handleRenderedKeydown(e: KeyboardEvent) {
    if (e.key !== "Enter" && e.key !== " ") return;
    const target = e.target as HTMLElement;
    const checkbox = clickedTaskCheckbox(target);
    if (!checkbox || checkbox.dataset.taskAction !== "done") return;
    e.stopPropagation();
    e.preventDefault();
    void handleTaskComplete();
  }

  function handleDateCancel() {
    showDatePicker = false;
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="block-item"
  class:bookMode
  class:editing={isEditing}
  class:selected
  class:code-block={isFenceBlock || isCodeBlock !== null}
  class:image-menu-open={imageSizeMenu !== null}
  class:h1={editorStyleClass === "h1"}
  class:h2={editorStyleClass === "h2"}
  class:h3={editorStyleClass === "h3"}
  class:h4={editorStyleClass === "h4"}
  class:h5={editorStyleClass === "h5"}
  class:h6={editorStyleClass === "h6"}
  class:table-block={isTableBlock && !isEditing}
  style="padding-left: {bookMode ? 0 : depth * 24}px"
  data-block-id={block.id}
  data-page-id={pageId}
  data-depth={depth}
  onpointerdown={markCurrentBlock}
>
  {#if !bookMode && showGuides && (guides.length > 0 || threadElbow || threadContinuationDepth !== null || threadStem)}
    <div class="indent-guides" aria-hidden="true">
      {#each guides as active, level (level)}
        {#if active && level !== threadContinuationDepth && !(threadElbow && level === depth - 1)}
          <span class="indent-guide-line" style={`left: ${level * 24 + 10}px`}></span>
        {/if}
      {/each}
      {#if threadElbow}
        <span
          class="indent-guide-elbow"
          style={`left: ${(depth - 1) * 24 + 9}px`}
        ></span>
      {/if}
      {#if threadContinuationDepth !== null}
        <span class="indent-guide-line indent-guide-path" style={`left: ${threadContinuationDepth * 24 + 9}px`}></span>
      {/if}
      {#if threadStem}
        <span class="indent-guide-line indent-guide-stem" style={`left: ${depth * 24 + 9}px`}></span>
      {/if}
    </div>
  {/if}
  {#if showBlockMarker}
    <div class="bullet-container" class:has-children={hasChildren} style={`min-height: ${bulletMinHeight};`} onclick={(e) => {
      e.stopPropagation();
      // A plain click on a bullet with children collapses/expands it (existing
      // behavior). But shift/ctrl/cmd-click should always select the block for
      // multi-select — otherwise header/parent blocks (which always have
      // children) could never be added to a selection, and the "Delete
      // selected" toolbar button would silently have nothing to act on.
      if (hasChildren && !e.shiftKey && !e.ctrlKey && !e.metaKey) {
        onToggleCollapse?.(block.id);
      } else {
        onBulletClick?.(block.id, e);
      }
    }}>
      {#if hasChildren}
        <span class="collapse-arrow" class:collapsed>
          {#if collapsed || suppressBullet}{collapsed ? "▶" : "▼"}{:else}
            <span class="arrow-hover">▼</span><span class="bullet-default">•</span>
          {/if}
        </span>
      {:else if !suppressBullet}
        <span class="bullet">•</span>
      {/if}
    </div>
  {/if}
  <div
    class="block-content"
    class:quote-block={isQuoteBlock && !isEditing}
    onclick={handleClick}
    bind:this={blockContentEl}
  >
    {#if isEditing}
      <div class="editor-shell">
        <div class="editor-wrapper" class:normal-block={editorStyleClass === "normal-block"} class:multiline-block={editorStyleClass === "multiline-block"} class:h1={editorStyleClass === "h1"} class:h2={editorStyleClass === "h2"} class:h3={editorStyleClass === "h3"} class:h4={editorStyleClass === "h4"} class:h5={editorStyleClass === "h5"} class:h6={editorStyleClass === "h6"} bind:this={editorContainer}></div>
        {#if saveError}
          <div class="save-error" role="alert">
            <span>{saveError}</span>
            <button class="save-retry" type="button" onclick={(e) => { e.stopPropagation(); void retrySave(); }}>
              Retry
            </button>
          </div>
        {/if}
      </div>
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
                {#each visibleQueryRows ?? [] as row}
                  <tr>
                    {#each row as [col, val], i}
                      {#if i !== queryBlockIdCol || col.toLowerCase() !== "_block_id"}
                        <td>
                          {#if col === "content" && val}
                            <span class="rendered-content query-cell-content">{@html renderBlock(String(val), queryRowBaseDir(row))}</span>
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
          {#if queryRows.length > queryRowLimit}
            <div class="query-more">
              <span>Showing {queryRowLimit} of {queryRows.length}</span>
              <button
                class="query-more-btn"
                onclick={(e) => { e.stopPropagation(); queryRowLimit += QUERY_ROW_PAGE; }}
              >Show more</button>
            </div>
          {/if}
        {/if}
      </div>
    {:else}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="rendered-content"
        onclick={handleRenderedClick}
        onkeydown={handleRenderedKeydown}
        oncontextmenu={handleRenderedContextMenu}
        bind:this={renderedEl}
      >
        {#if isVisuallyEmpty}
          <span class="placeholder">&nbsp;</span>
        {:else}
          {@html renderedHtml}
        {/if}
      </div>
      {#if saveError || imageMenuMessage}
        <div class="save-error rendered-action-message" role="status">
          <span>{saveError ?? imageMenuMessage}</span>
        </div>
      {/if}
    {/if}
    {#each pendingPastes as paste (paste.id)}
      <div class="paste-preview">
        {#if paste.error}
          <div class="save-error" role="alert">
            Paste could not be completed: {paste.error}. Saving all of this text could not be confirmed;
            copy it before leaving this page.
          </div>
          <textarea aria-label="Unfinished pasted text" readonly rows="10" value={paste.markdown}
            onclick={(event) => event.stopPropagation()}></textarea>
        {:else}
          <div class="paste-status" role="status">Saving pasted blocks...</div>
          <pre>{paste.markdown}</pre>
        {/if}
      </div>
    {/each}
    {#if imageSizeMenu}
      <div
        class="image-size-menu app-context-menu"
        style={`left: ${imageSizeMenu.x}px; top: ${imageSizeMenu.y}px;`}
        role="menu"
        aria-label="Image menu"
        tabindex="-1"
        onpointerdown={(e) => e.stopPropagation()}
        oncontextmenu={(e) => { e.stopPropagation(); e.preventDefault(); }}
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => { if (e.key === "Escape") imageSizeMenu = null; }}
      >
        <div class="image-size-menu-title">Image</div>
        <button class="image-size-menu-item" type="button" role="menuitem" onclick={() => copyImage()}>
          Copy Image
        </button>
        <button class="image-size-menu-item" type="button" role="menuitem" onclick={() => copyImageAddress()}>
          Copy Image Address
        </button>
        <button class="image-size-menu-item" type="button" role="menuitem" onclick={() => copyMarkdownImage()}>
          Copy Markdown Image
        </button>
        <button class="image-size-menu-item" type="button" role="menuitem" onclick={() => openImageExternally()}>
          Open Image
        </button>
        <button class="image-size-menu-item" type="button" role="menuitem" onclick={() => saveImageAs()}>
          Save Image As...
        </button>
        <div class="image-size-menu-separator"></div>
        <div class="image-size-menu-title">Size</div>
        <button class="image-size-menu-item" type="button" role="menuitem" onclick={() => resetImageScale()}>
          Original Size
        </button>
        {#each IMAGE_SIZE_SCALES as scale}
          <button class="image-size-menu-item" type="button" role="menuitem" onclick={() => chooseImageScale(scale)}>
            {formatImageScale(scale)} <span>{imageScaleSizeLabel(scale)}</span>
          </button>
        {/each}
      </div>
    {/if}
    {#if makeLinkMenu}
      <div
        class="make-link-menu app-context-menu"
        style={`left: ${makeLinkMenu.x}px; top: ${makeLinkMenu.y}px;`}
        role="menu"
        aria-label="Text selection menu"
        tabindex="-1"
        onpointerdown={(e) => { e.stopPropagation(); e.preventDefault(); }}
        oncontextmenu={(e) => { e.stopPropagation(); e.preventDefault(); }}
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => { if (e.key === "Escape") makeLinkMenu = null; }}
      >
        <button class="make-link-menu-item" type="button" role="menuitem" onclick={() => void makeSelectedTextLink()}>
          Make link
        </button>
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

{#if isEditing}
  <MobileEditorBar
    onTodo={() => void handleMobileTodo()}
    onOutdent={() => handleMobileIndent("out")}
    onIndent={() => handleMobileIndent("in")}
    onLink={handleMobileLink}
    onTag={() => handleMobileInsert("#")}
    onSlash={() => handleMobileInsert("/", true)}
    onHide={() => void stopEditing()}
  />
{/if}

<style>
  .block-item {
    display: flex;
    align-items: center;
    min-height: 24px;
    min-width: 0;
    padding-bottom: 2px;
    box-sizing: border-box;
    border-radius: 4px;
    transition: background-color 0.1s;
    scroll-margin: 40px;
    position: relative;
    overflow: visible;
  }

  .block-item.image-menu-open {
    z-index: 2000;
  }

  .block-item.editing {
    background: transparent;
    align-items: center;
  }

  @media (max-width: 640px) {
    .block-item.editing {
      scroll-margin-bottom: 96px;
    }
  }

  .block-item.selected {
    background: var(--accent, #7c3aed);
    background: color-mix(in srgb, var(--accent, #7c3aed) 20%, transparent);
  }

  .block-item.bookMode {
    min-height: 0;
    padding-bottom: 0;
    border-radius: 0;
    transition: none;
  }

  .block-item.bookMode:not(.editing) {
    margin: 0.24rem 0;
  }

  .block-item.bookMode.h1:not(.editing) {
    margin: 2.2rem 0 0.95rem;
  }

  .block-item.bookMode.h2:not(.editing) {
    margin: 1.65rem 0 0.65rem;
  }

  .block-item.bookMode.h3:not(.editing),
  .block-item.bookMode.h4:not(.editing),
  .block-item.bookMode.h5:not(.editing),
  .block-item.bookMode.h6:not(.editing) {
    margin: 1.25rem 0 0.45rem;
  }

  .bullet-container {
    width: 20px;
    min-height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    cursor: pointer;
    font-size: 15px;
    position: relative;
    z-index: 1;
  }

  /* Bullet centers track the row height, excluding its 2px bottom spacing.
     Keep the lines inside that spacing so adjacent rows meet without clipping. */
  .indent-guides {
    position: absolute;
    inset: 0;
    overflow: visible;
    pointer-events: none;
    z-index: 0;
  }

  .indent-guide-line {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1px;
    border-radius: 0;
    background: color-mix(in srgb, var(--text-muted) 55%, transparent);
  }

  .indent-guide-path {
    width: 2px;
    background: var(--accent);
  }

  .indent-guide-stem {
    top: calc(50% - 1px);
    width: 2px;
    background: var(--accent);
  }

  .indent-guide-elbow {
    position: absolute;
    box-sizing: border-box;
    top: 0;
    width: 25px;
    height: 50%;
    border-left: 2px solid var(--accent);
    border-bottom: 2px solid var(--accent);
    border-bottom-left-radius: 8px;
  }

  .bullet {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: color-mix(in srgb, var(--text-secondary) 78%, var(--accent));
    display: block;
    font-size: 0;
  }

  .collapse-arrow {
    font-size: 1em;
    color: var(--text-muted);
    user-select: none;
    line-height: 1;
    font-weight: 700;
  }

  .bullet-container.has-children .collapse-arrow {
    color: var(--accent);
    text-shadow: 0 0 4px color-mix(in srgb, var(--accent) 35%, transparent);
  }

  .block-item.h1 .bullet-container {
    font-size: 1.75em;
  }

  .block-item.h1 .bullet-container.has-children .collapse-arrow {
    color: var(--accent-yellow);
  }

  .block-item.h2 .bullet-container {
    font-size: 1.45em;
  }

  .block-item.h2 .bullet-container.has-children .collapse-arrow {
    color: var(--accent);
  }

  .block-item.h3 .bullet-container {
    font-size: 1.25em;
  }

  .block-item.h3 .bullet-container.has-children .collapse-arrow {
    color: var(--accent-secondary);
  }

  .block-item.h4 .bullet-container,
  .block-item.h5 .bullet-container,
  .block-item.h6 .bullet-container {
    font-size: 1.08em;
  }

  .collapse-arrow .arrow-hover {
    display: none;
    font-size: 1em;
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
    font-size: 1em;
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

  .block-item.bookMode .block-content {
    min-height: 0;
    overflow: visible;
    font-family: Georgia, "Times New Roman", serif;
    font-size: 17px;
    line-height: 1.72;
    color: var(--text-primary);
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

  .editor-shell {
    width: 100%;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .paste-preview {
    margin-top: 8px;
  }

  .paste-status {
    color: var(--text-muted);
    font-size: 12px;
  }

  .paste-preview pre,
  .paste-preview textarea {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font: inherit;
  }

  .paste-preview textarea {
    width: 100%;
  }

  .save-error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 10px;
    border-radius: 6px;
    border: 1px solid rgba(243, 139, 168, 0.35);
    background: rgba(243, 139, 168, 0.12);
    color: #f38ba8;
    font-size: 12px;
  }

  .save-retry {
    border: none;
    border-radius: 4px;
    background: rgba(243, 139, 168, 0.18);
    color: inherit;
    padding: 4px 8px;
    cursor: pointer;
    font-size: 12px;
    font-weight: 600;
  }

  .save-retry:hover {
    background: rgba(243, 139, 168, 0.28);
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
    color: var(--accent-yellow);
    font-size: 1.75em;
    line-height: 1.08;
    font-weight: 800;
  }

  .editor-wrapper.h2 :global(.cm-editor) {
    color: var(--accent);
    font-size: 1.45em;
    line-height: 1.14;
    font-weight: 750;
  }

  .editor-wrapper.h3 :global(.cm-editor) {
    color: var(--accent-secondary);
    font-size: 1.25em;
    line-height: 1.25;
    font-weight: 700;
  }

  .editor-wrapper.h4 :global(.cm-editor),
  .editor-wrapper.h5 :global(.cm-editor),
  .editor-wrapper.h6 :global(.cm-editor) {
    color: var(--accent-cyan);
    font-size: 1.08em;
    line-height: 1.25;
    font-weight: 700;
  }

  .rendered-content {
    position: relative;
    width: 100%;
    min-width: 0;
    padding: 0;
    overflow-x: auto;
    overflow-wrap: break-word;
    word-break: break-word;
  }

  .block-item.bookMode .rendered-content {
    overflow-x: visible;
    overflow-wrap: break-word;
    word-break: normal;
    hyphens: auto;
  }

  .rendered-content :global(p) {
    margin: 0;
  }

  .block-item.bookMode .rendered-content :global(p) {
    margin: 0;
  }

  .placeholder {
    color: var(--text-muted);
    font-style: italic;
  }

  .rendered-content :global(.page-link) {
    --link-accent: var(--accent-yellow);
    color: var(--link-accent);
    cursor: pointer;
    text-decoration: none;
    font-family: inherit;
    font-size: inherit;
    font-weight: inherit;
    line-height: inherit;
    border: 1px solid color-mix(in srgb, var(--link-accent) 50%, transparent);
    border-radius: 5px;
    background: color-mix(in srgb, var(--link-accent) 10%, transparent);
    box-decoration-break: clone;
    -webkit-box-decoration-break: clone;
    padding: 0 0.24em;
  }

  .rendered-content :global(.page-link:hover) {
    color: var(--text-link-hover);
    border-color: var(--text-link-hover);
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
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 2px 8px;
    border: 1px solid currentColor;
    border-radius: 999px;
    font-size: 0.78rem;
    font-weight: 800;
    line-height: 1.25;
    letter-spacing: 0.03em;
    margin-right: 6px;
    vertical-align: 1px;
    cursor: pointer;
    user-select: none;
    transition: opacity 0.15s, background-color 0.15s, box-shadow 0.15s;
  }

  .rendered-content :global(.task-marker:hover) {
    opacity: 0.78;
  }

  .rendered-content :global(.task-marker.todo) {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 15%, transparent);
  }

  .rendered-content :global(.task-marker.doing),
  .rendered-content :global(.task-marker.now) {
    color: var(--accent-secondary);
    background: color-mix(in srgb, var(--accent-secondary) 15%, transparent);
  }

  .rendered-content :global(.task-marker.done) {
    color: var(--task-done-fg);
    background: var(--task-done-bg);
  }

  .rendered-content :global(.task-marker.later) {
    color: var(--text-muted);
    background: var(--bg-secondary);
  }

  .rendered-content :global(.task-marker.canceled) {
    color: var(--text-muted);
    background: color-mix(in srgb, var(--text-muted) 16%, transparent);
    text-decoration: line-through;
  }

  .rendered-content :global(.priority) {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 2px 8px;
    border: 1px solid currentColor;
    border-radius: 999px;
    font-size: 0.78rem;
    font-weight: 800;
    line-height: 1.25;
    letter-spacing: 0.03em;
    margin-right: 6px;
    vertical-align: 1px;
  }

  .rendered-content :global(.priority-A) {
    color: var(--danger, #f85149);
    background: color-mix(in srgb, var(--danger, #f85149) 14%, transparent);
  }

  .rendered-content :global(.priority-B) {
    color: var(--accent-yellow);
    background: color-mix(in srgb, var(--accent-yellow) 14%, transparent);
  }

  .rendered-content :global(.priority-C) {
    color: var(--accent-cyan);
    background: color-mix(in srgb, var(--accent-cyan) 12%, transparent);
  }

  .rendered-content :global(.task-date) {
    display: inline-flex;
    align-items: center;
    padding: 2px 7px;
    border-radius: 999px;
    font-size: 0.76rem;
    font-weight: 650;
    line-height: 1.25;
    margin: 2px 4px 0 0;
    vertical-align: 1px;
  }

  .rendered-content :global(.task-date.scheduled) {
    background: color-mix(in srgb, var(--accent-cyan) 12%, transparent);
    color: var(--text-secondary);
  }

  .rendered-content :global(.task-date.deadline) {
    background: color-mix(in srgb, var(--danger, #f85149) 12%, transparent);
    color: var(--danger, #f85149);
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
    --heading-accent: var(--accent-yellow);
    color: var(--heading-accent);
    font-size: 1.75em;
    font-weight: 800;
    line-height: 1.08;
    letter-spacing: 0.015em;
    margin: 0;
    padding-bottom: 0.08em;
    border-bottom: 2px solid color-mix(in srgb, var(--heading-accent) 62%, transparent);
  }

  .block-item.bookMode .rendered-content :global(h1) {
    font-size: clamp(1.75rem, 4vw, 2.6rem);
    font-weight: 700;
    line-height: 1.12;
    margin: 0;
    text-align: center;
  }

  .rendered-content :global(h2) {
    --heading-accent: var(--accent);
    color: var(--heading-accent);
    font-size: 1.45em;
    font-weight: 750;
    line-height: 1.14;
    letter-spacing: 0.01em;
    margin: 0;
    padding-bottom: 0.06em;
    border-bottom: 1px solid color-mix(in srgb, var(--heading-accent) 52%, transparent);
  }

  .block-item.bookMode .rendered-content :global(h2) {
    font-size: 1.65rem;
    font-weight: 700;
    line-height: 1.18;
    margin: 0;
    text-align: center;
  }

  .rendered-content :global(h3) {
    --heading-accent: var(--accent-secondary);
    color: var(--heading-accent);
    font-size: 1.25em;
    font-weight: 700;
    line-height: 1.25;
    margin: 0;
  }

  .block-item.bookMode .rendered-content :global(h3) {
    font-size: 1.3rem;
    font-weight: 700;
    line-height: 1.25;
    margin: 0;
    text-align: center;
  }

  .rendered-content :global(h4),
  .rendered-content :global(h5),
  .rendered-content :global(h6) {
    --heading-accent: var(--accent-cyan);
    color: var(--heading-accent);
    font-size: 1.08em;
    font-weight: 700;
    line-height: 1.25;
    margin: 0;
  }

  .block-item.bookMode .rendered-content :global(h4),
  .block-item.bookMode .rendered-content :global(h5),
  .block-item.bookMode .rendered-content :global(h6) {
    font-size: 1.05rem;
    font-weight: 700;
    line-height: 1.3;
    margin: 0;
    text-align: center;
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
    height: auto;
    border-radius: 6px;
    margin: 4px 0;
    display: block;
    cursor: pointer;
  }

  .rendered-content :global(.fc-img[data-src]:not([src])) {
    min-height: 48px;
    background: color-mix(in srgb, var(--text-muted) 8%, transparent);
  }

  .rendered-content :global(.fc-img[data-image-width]),
  .rendered-content :global(.fc-img[data-image-height]) {
    max-width: none;
  }

  .rendered-action-message {
    margin-top: 4px;
  }

  .image-size-menu,
  .make-link-menu {
    position: fixed;
    z-index: 2147483000;
    padding: 6px;
  }

  .image-size-menu {
    min-width: 212px;
  }

  .make-link-menu {
    min-width: 150px;
  }

  .image-size-menu-title {
    padding: 5px 8px 6px;
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .image-size-menu-separator {
    height: 1px;
    margin: 5px 2px;
    background: var(--border);
  }

  .image-size-menu-item,
  .make-link-menu-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    width: 100%;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
    font: inherit;
    font-size: 13px;
    padding: 7px 8px;
    text-align: left;
  }

  .image-size-menu-item span {
    color: var(--text-muted);
    font-size: 11px;
  }

  .image-size-menu-item:hover,
  .image-size-menu-item:focus-visible,
  .make-link-menu-item:hover,
  .make-link-menu-item:focus-visible {
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    color: var(--accent);
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

  .rendered-content :global(.grafium-video-embed) {
    width: min(100%, 760px);
    aspect-ratio: 16 / 9;
    margin: 8px 0;
    border: 1px solid var(--border);
    border-radius: 10px;
    overflow: hidden;
    background: var(--bg-secondary);
  }

  .rendered-content :global(.grafium-video-embed iframe) {
    width: 100%;
    height: 100%;
    border: 0;
    display: block;
  }

  .rendered-content :global(ul),
  .rendered-content :global(ol) {
    margin: 0;
    padding-left: 0;
    list-style-position: inside;
  }

  .rendered-content :global(li:has(.code-block-wrapper)) {
    list-style: none;
  }

  .block-item.code-block .bullet-container {
    display: none;
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
    cursor: pointer;
    user-select: none;
  }

  .rendered-content :global(th[aria-sort="ascending"])::after,
  .rendered-content :global(th[aria-sort="descending"])::after {
    content: " ▲";
    font-size: 0.75em;
    color: var(--accent);
  }

  .rendered-content :global(th[aria-sort="descending"])::after {
    content: " ▼";
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

  .query-more {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border-top: 1px solid var(--border);
    color: var(--text-muted);
    font-size: 12px;
  }

  .query-more-btn {
    background: none;
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 3px 10px;
    color: var(--text);
    font-size: 12px;
    cursor: pointer;
  }

  .query-more-btn:hover {
    background: var(--bg-hover);
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
    background-color: var(--bg-primary) !important;
    background-image: linear-gradient(var(--bg-secondary), var(--bg-secondary)) !important;
    opacity: 1 !important;
    border: 1px solid var(--border) !important;
    border-radius: 6px !important;
    box-shadow: 0 8px 24px rgba(0,0,0,0.45) !important;
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
