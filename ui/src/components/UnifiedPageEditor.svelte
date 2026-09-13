<script lang="ts">
  import { onDestroy, tick } from "svelte";
  import { autocompletion } from "@codemirror/autocomplete";
  import {
    EditorSelection,
    EditorState,
    Compartment,
    Prec,
    RangeSetBuilder,
    StateEffect,
    StateField,
    type Transaction,
  } from "@codemirror/state";
  import {
    Decoration,
    type DecorationSet,
    drawSelection,
    EditorView,
    keymap,
    WidgetType,
  } from "@codemirror/view";
  import {
    defaultKeymap,
    history,
    historyKeymap,
  } from "@codemirror/commands";
  import { markdown } from "@codemirror/lang-markdown";
  import { applyBionicReaderToElement, bionicReaderEnabled, isBionicReaderEnabled } from "../lib/bionicReader";
  import { emojiIconCompletionSource } from "../lib/emojiIconCompletion";
  import type { Page } from "../lib/api";
  import { getPageSource, updatePageSource } from "../lib/api";
  import {
    assetBaseDirFor,
    clearMarkdownImageWidth,
    hydrateAssetMedia,
    renderBlock,
    setMarkdownImageWidth,
  } from "../lib/markdown";
  import { contextMenuPositionFromEvent } from "../lib/contextMenu";
  import {
    formatImageScale,
    formatScaledImageDimensions,
    imageIndexFromElement,
    IMAGE_SIZE_SCALES,
    renderedImageBaseSize,
    scaledImageDimensions,
  } from "../lib/imageSizing";
  import { EDITOR_UNDO_MIN_DEPTH } from "../lib/editorUndo";
  import {
    parsePageSourceMap,
    sourceBlockContentReplacement,
    type SourceBlock,
  } from "../lib/pageSourceMap";
  import { isFencedCodeBlock } from "../lib/codeFence";

  interface Props {
    page: Page;
    compact?: boolean;
    onReload?: () => void | Promise<void>;
    onExitPrototype?: () => void;
  }

  let { page, compact = false, onReload, onExitPrototype }: Props = $props();

  let editorHost: HTMLDivElement;
  let editorView: EditorView | undefined;
  let verticalArrowCleanup: (() => void) | undefined;
  let loadToken = 0;
  let loading = $state(false);
  let saving = $state(false);
  let dirty = $state(false);
  let error: string | null = $state(null);
  let savedMessage = $state("");
  let assetBaseDir = $derived(assetBaseDirFor(page.file_path));

  interface UnifiedImageMenuDetail {
    blockId: string | null;
    blockPreviewFrom: number;
    blockContent: string;
    imageIndex: number;
    clientX: number;
    clientY: number;
    baseWidth: number;
    baseHeight: number;
  }

  type ImageSizeMenu = UnifiedImageMenuDetail & {
    x: number;
    y: number;
  };

  let imageSizeMenu: ImageSizeMenu | null = $state(null);

  function sourceBlockKey(block: SourceBlock): string {
    return block.id ?? `pos:${block.previewFrom}`;
  }

  class RenderedBlockWidget extends WidgetType {
    private blockKey: string;
    private blockId: string | null;
    private depth: number;
    private content: string;
    private editPos: number;
    private previewFrom: number;
    private assetBaseDir: string;
    private bionic: boolean;

    constructor(block: SourceBlock, assetBaseDir: string) {
      super();
      this.blockKey = sourceBlockKey(block);
      this.blockId = block.id;
      this.depth = block.depth;
      this.content = block.content;
      this.editPos = block.contentFrom;
      this.previewFrom = block.previewFrom;
      this.assetBaseDir = assetBaseDir;
      this.bionic = isBionicReaderEnabled();
    }

    eq(other: WidgetType): boolean {
      return other instanceof RenderedBlockWidget
        && other.blockKey === this.blockKey
        && other.blockId === this.blockId
        && other.depth === this.depth
        && other.content === this.content
        && other.editPos === this.editPos
        && other.previewFrom === this.previewFrom
        && other.assetBaseDir === this.assetBaseDir
        && other.bionic === this.bionic;
    }

    toDOM(view: EditorView): HTMLElement {
      const row = document.createElement("div");
      row.className = "unified-rendered-block";
      row.style.setProperty("--preview-depth", String(this.depth));
      row.title = "Click to edit this block as raw Markdown";
      if (this.blockId) {
        row.dataset.sourceBlockId = this.blockId;
      }

      const content = document.createElement("div");
      content.className = "unified-rendered-content rendered-content";
      if (this.content.trim()) {
        content.innerHTML = renderBlock(this.content, this.assetBaseDir);
      } else {
        content.appendChild(document.createTextNode("\u00a0"));
      }

      if (isFencedCodeBlock(this.content)) {
        row.classList.add("code-block");
        row.style.gridTemplateColumns = "minmax(0, 1fr)";
        row.append(content);
      } else {
        const bullet = document.createElement("span");
        bullet.className = "unified-rendered-bullet";
        bullet.textContent = "•";
        row.append(bullet, content);
      }
      row.addEventListener("click", (event) => {
        if (event.button !== 0) return;
        if (row.ownerDocument.getSelection()?.toString()) return;
        row.ownerDocument.getSelection()?.removeAllRanges();
        event.preventDefault();
        event.stopPropagation();
        view.dispatch({
          selection: EditorSelection.cursor(this.editPos),
          effects: activeBlockEffects(this.blockKey),
          scrollIntoView: true,
          userEvent: "select.pointer",
        });
        view.focus();
      });
      row.addEventListener("contextmenu", (event) => {
        const target = event.target as Element | null;
        const img = target instanceof HTMLImageElement
          ? target
          : target?.closest?.("img.fc-img") as HTMLImageElement | null;
        if (!img?.classList.contains("fc-img")) return;
        const imageIndex = imageIndexFromElement(img);
        if (imageIndex === null) return;
        const baseSize = renderedImageBaseSize(img, content.clientWidth);
        event.preventDefault();
        event.stopPropagation();
        row.dispatchEvent(new CustomEvent<UnifiedImageMenuDetail>("unified-image-context-menu", {
          bubbles: true,
          detail: {
            blockId: this.blockId,
            blockPreviewFrom: this.previewFrom,
            blockContent: this.content,
            imageIndex,
            clientX: event.clientX,
            clientY: event.clientY,
            baseWidth: baseSize.width,
            baseHeight: baseSize.height,
          },
        }));
      });
      queueMicrotask(() => {
        if (this.bionic) applyBionicReaderToElement(content);
        void hydrateAssetMedia(content);
      });
      return row;
    }

    ignoreEvent(): boolean {
      return true;
    }

    get estimatedHeight(): number {
      return 26;
    }
  }

  function sourceLineWithBreakTo(sourceMap: ReturnType<typeof parsePageSourceMap>, lineIndex: number): number {
    const line = sourceMap.lines[lineIndex];
    if (!line) return 0;
    return line.index + 1 < sourceMap.lines.length ? line.to + 1 : line.to;
  }

  function blockPreviewWithBreakTo(sourceMap: ReturnType<typeof parsePageSourceMap>, block: SourceBlock): number {
    const lastLine = block.contentSegments.at(-1)?.line ?? block.line;
    return sourceLineWithBreakTo(sourceMap, lastLine.index);
  }

  const activeBlockEffect = StateEffect.define<string | null>();
  const bionicModeEffect = StateEffect.define<void>();

  const activeBlockField = StateField.define<string | null>({
    create() {
      return null;
    },
    update(activeBlock, transaction) {
      for (const effect of transaction.effects) {
        if (effect.is(activeBlockEffect)) {
          return effect.value;
        }
      }
      return activeBlock;
    },
  });

  const editingModeCompartment = new Compartment();

  function editingModeExtensions(active: boolean) {
    return [
      EditorView.editable.of(active),
      EditorState.readOnly.of(!active),
    ];
  }

  function activeBlockEffects(blockKey: string | null) {
    return [
      activeBlockEffect.of(blockKey),
      editingModeCompartment.reconfigure(editingModeExtensions(blockKey !== null)),
    ];
  }

  function setActiveBlock(view: EditorView, blockKey: string | null) {
    if (view.state.field(activeBlockField, false) === blockKey) return;
    view.dispatch({
      effects: activeBlockEffects(blockKey),
    });
  }

  function transactionChangesActiveBlock(transaction: Transaction): boolean {
    return transaction.effects.some((effect) => effect.is(activeBlockEffect));
  }

  function buildBlockPreviewDecorations(state: EditorState): DecorationSet {
    const builder = new RangeSetBuilder<Decoration>();
    const sourceMap = parsePageSourceMap(state.doc.toString());
    const activeBlockKey = state.field(activeBlockField, false);
    const activeBlock = activeBlockKey
      ? sourceMap.blocks.find((block) => sourceBlockKey(block) === activeBlockKey) ?? null
      : null;
    const activeEditing = activeBlock !== null;
    const ranges: Array<{ from: number; to: number; decoration: Decoration }> = [];

    for (const idLine of sourceMap.idLines) {
      const isActiveIdLine = activeEditing && activeBlock?.idLine === idLine;
      if (isActiveIdLine) {
        ranges.push({
          from: idLine.from,
          to: idLine.from,
          decoration: Decoration.line({
            class: "cm-id-property-line cm-id-property-line-active",
          }),
        });
      } else {
        ranges.push({
          from: idLine.from,
          to: sourceLineWithBreakTo(sourceMap, idLine.line.index),
          decoration: Decoration.replace({
            block: true,
          }),
        });
      }
    }

    for (const block of sourceMap.blocks) {
      if (activeEditing && block === activeBlock) {
        continue;
      }
      ranges.push({
        from: block.previewFrom,
        to: blockPreviewWithBreakTo(sourceMap, block),
        decoration: Decoration.replace({
          widget: new RenderedBlockWidget(block, assetBaseDir),
          block: true,
          inclusive: false,
        }),
      });
    }

    ranges.sort((a, b) => a.from - b.from || a.to - b.to);
    for (const range of ranges) {
      builder.add(range.from, range.to, range.decoration);
    }

    return builder.finish();
  }

  const blockPreviewField = StateField.define<DecorationSet>({
    create(state) {
      return buildBlockPreviewDecorations(state);
    },
    update(decorations, transaction) {
      return transaction.docChanged || transactionChangesActiveBlock(transaction)
        || transaction.effects.some((effect) => effect.is(bionicModeEffect))
        ? buildBlockPreviewDecorations(transaction.state)
        : decorations;
    },
    provide(field) {
      return EditorView.decorations.from(field);
    },
  });

  $effect(() => {
    const pageId = page.id;
    void loadSource(pageId);
  });

  $effect(() => bionicReaderEnabled.subscribe(() => {
    editorView?.dispatch({ effects: bionicModeEffect.of(undefined) });
  }));

  onDestroy(() => {
    destroyEditor();
  });

  $effect(() => {
    const host = editorHost;
    if (!host) return;

    const handler = (event: Event) => {
      openImageSizeMenu(event as CustomEvent<UnifiedImageMenuDetail>);
    };
    host.addEventListener("unified-image-context-menu", handler);
    return () => host.removeEventListener("unified-image-context-menu", handler);
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

  function errorMessage(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
  }

  function openImageSizeMenu(event: CustomEvent<UnifiedImageMenuDetail>) {
    const detail = event.detail;
    if (!detail) return;
    const pos = contextMenuPositionFromEvent(detail, { width: 236, height: 360 });
    imageSizeMenu = {
      ...detail,
      x: pos.x,
      y: pos.y,
    };
  }

  function imageScaleSizeLabel(scale: number): string {
    const baseWidth = imageSizeMenu?.baseWidth ?? 240;
    const baseHeight = imageSizeMenu?.baseHeight ?? 180;
    return formatScaledImageDimensions(scale, baseWidth, baseHeight);
  }

  function findImageMenuBlock(menu: ImageSizeMenu): SourceBlock | null {
    if (!editorView) return null;
    const map = parsePageSourceMap(editorView.state.doc.toString());
    return (
      (menu.blockId ? map.blocks.find((block) => block.id === menu.blockId) : null)
      ?? map.blocks.find((block) => block.previewFrom === menu.blockPreviewFrom && block.content === menu.blockContent)
      ?? map.blocks.find((block) => block.content === menu.blockContent)
      ?? null
    );
  }

  function replaceImageMenuBlockContent(menu: ImageSizeMenu, content: string) {
    if (!editorView) throw new Error("editor is not ready");
    const block = findImageMenuBlock(menu);
    if (!block) throw new Error("could not find the image block in page source");
    if (content === block.content) return;

    const replacement = sourceBlockContentReplacement(block, content);
    editorView.dispatch({
      changes: replacement,
      userEvent: "input.resize-image",
    });
    editorView.focus();
  }

  function chooseImageScale(scale: number) {
    const menu = imageSizeMenu;
    if (!menu) return;
    try {
      const nextSize = scaledImageDimensions(scale, menu.baseWidth, menu.baseHeight);
      replaceImageMenuBlockContent(menu, setMarkdownImageWidth(menu.blockContent, menu.imageIndex, nextSize.width));
      imageSizeMenu = null;
      error = null;
    } catch (e) {
      error = `Failed to resize image: ${errorMessage(e)}`;
      console.error("Failed to resize image:", e);
    }
  }

  function resetImageScale() {
    const menu = imageSizeMenu;
    if (!menu) return;
    try {
      replaceImageMenuBlockContent(menu, clearMarkdownImageWidth(menu.blockContent, menu.imageIndex));
      imageSizeMenu = null;
      error = null;
    } catch (e) {
      error = `Failed to reset image size: ${errorMessage(e)}`;
      console.error("Failed to reset image size:", e);
    }
  }

  function destroyEditor() {
    verticalArrowCleanup?.();
    verticalArrowCleanup = undefined;
    if (editorView && (window as any).__activeEditorView === editorView) {
      (window as any).__activeEditorView = undefined;
    }
    if (editorView && (window as any).__unifiedPageEditorView === editorView) {
      (window as any).__unifiedPageEditorView = undefined;
    }
    editorView?.destroy();
    editorView = undefined;
  }

  function resetEditor(content: string) {
    destroyEditor();
    if (!editorHost) return;

    const state = EditorState.create({
      doc: content,
      extensions: [
        markdown(),
        history({ minDepth: EDITOR_UNDO_MIN_DEPTH }),
        autocompletion({
          override: [emojiIconCompletionSource],
          activateOnTyping: true,
          closeOnBlur: false,
        }),
        drawSelection(),
        editingModeCompartment.of(editingModeExtensions(false)),
        activeBlockField,
        blockPreviewField,
        Prec.highest(keymap.of([
          {
            key: "Shift-ArrowUp",
            run: (view) => moveVerticalSelection(view, "up", true),
            preventDefault: true,
          },
          {
            key: "Shift-ArrowDown",
            run: (view) => moveVerticalSelection(view, "down", true),
            preventDefault: true,
          },
        ])),
        keymap.of([
          {
            key: "Mod-s",
            run: () => {
              void saveSource();
              return true;
            },
          },
          // TODO(continuous-editor): replace native Enter/Backspace with
          // block-aware split/merge only after new `id::` line generation is
          // covered well enough to avoid corrupting source metadata.
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        EditorView.lineWrapping,
        EditorView.updateListener.of((update) => {
          if (update.docChanged) {
            dirty = true;
            savedMessage = "";
          }
        }),
        EditorView.domEventHandlers({
          focus: (_event, view) => {
            activateEditor(view);
          },
          pointerdown: (_event, view) => {
            activateEditor(view);
            const target = _event.target as Element | null;
            if (!target?.closest(".unified-rendered-block") && !target?.closest(".cm-line")) {
              setActiveBlock(view, null);
            }
          },
          blur: (_event, view) => {
            if ((window as any).__activeEditorView === view) {
              (window as any).__activeEditorView = undefined;
            }
            setActiveBlock(view, null);
          },
        }),
        EditorView.theme({
          "&": {
            fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', sans-serif",
            fontSize: "16px",
            lineHeight: "1.3",
            color: "var(--text-primary)",
            background: "transparent",
            fontWeight: "400",
          },
          "&.cm-editor": {
            border: "1px solid var(--border)",
            borderRadius: "8px",
            background: "var(--bg-secondary)",
          },
          "&.cm-focused": {
            outline: "1px solid var(--accent)",
          },
          ".cm-scroller": {
            fontFamily: "inherit",
            overflow: "visible",
            maxHeight: "none",
          },
          ".cm-content": {
            padding: "0",
            caretColor: "var(--text-primary)",
            minHeight: compact ? "120px" : "240px",
            fontFamily: "inherit",
            fontSize: "inherit",
            lineHeight: "inherit",
            fontWeight: "inherit",
          },
          ".cm-line": {
            padding: "0",
            fontFamily: "inherit",
            fontSize: "inherit",
            lineHeight: "inherit",
            fontWeight: "inherit",
          },
          ".cm-id-property-line": {
            color: "var(--text-muted)",
            fontSize: "0.86em",
            opacity: "0.58",
          },
          ".cm-id-property-line-active": {
            display: "block",
          },
          ".cm-gutters": {
            background: "var(--bg-secondary)",
            borderRight: "1px solid var(--border)",
            color: "var(--text-muted)",
          },
          ".cm-activeLineGutter": {
            background: "var(--bg-hover)",
          },
          ".cm-activeLine": {
            background: "color-mix(in srgb, var(--accent, #7c3aed) 8%, transparent)",
          },
          ".cm-cursor": {
            borderLeftColor: "var(--text-primary)",
          },
          ".cm-selectionBackground": {
            background: "rgba(124, 58, 237, 0.38) !important",
          },
          "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground": {
            background: "rgba(124, 58, 237, 0.46) !important",
          },
          "& ::selection": {
            background: "color-mix(in srgb, var(--accent, #7c3aed) 42%, transparent)",
            color: "var(--text-primary)",
          },
          "& *::selection": {
            background: "color-mix(in srgb, var(--accent, #7c3aed) 42%, transparent)",
            color: "var(--text-primary)",
          },
          ".unified-rendered-block": {
            display: "grid",
            gridTemplateColumns: "20px minmax(0, 1fr)",
            columnGap: "0",
            alignItems: "baseline",
            minHeight: "21px",
            marginBottom: "1px",
            paddingLeft: "calc(var(--preview-depth, 0) * 24px)",
            color: "var(--text-primary)",
            fontFamily: "inherit",
            fontSize: "inherit",
            lineHeight: "inherit",
            fontWeight: "inherit",
            whiteSpace: "normal",
            userSelect: "text",
            cursor: "text",
          },
          ".unified-rendered-bullet": {
            color: "var(--text-muted)",
            lineHeight: "inherit",
            userSelect: "none",
          },
          ".unified-rendered-content": {
            minWidth: "0",
            padding: "0",
            lineHeight: "inherit",
            overflowX: "auto",
            overflowWrap: "break-word",
            wordBreak: "break-word",
          },
          ".unified-rendered-content p, .unified-rendered-content ul, .unified-rendered-content ol, .unified-rendered-content li": {
            margin: "0",
            lineHeight: "inherit",
          },
          ".unified-rendered-content ul, .unified-rendered-content ol": {
            paddingLeft: "0",
            listStylePosition: "inside",
          },
          ".unified-rendered-content li:has(.code-block-wrapper)": {
            listStyle: "none",
          },
          ".unified-rendered-content li > p": {
            display: "inline",
          },
          ".unified-rendered-content > :first-child": {
            marginTop: "0",
          },
          ".unified-rendered-content > :last-child": {
            marginBottom: "0",
          },
          ".unified-rendered-content h1, .unified-rendered-content h2, .unified-rendered-content h3": {
            margin: "0",
          },
          ".unified-rendered-content h1": {
            "--heading-accent": "var(--accent-yellow)",
            color: "var(--heading-accent)",
            fontSize: "1.75em",
            fontWeight: "800",
            lineHeight: "1.08",
            letterSpacing: "0.015em",
            paddingBottom: "0.08em",
            borderBottom: "2px solid color-mix(in srgb, var(--heading-accent) 62%, transparent)",
          },
          ".unified-rendered-content h2": {
            "--heading-accent": "var(--accent)",
            color: "var(--heading-accent)",
            fontSize: "1.45em",
            fontWeight: "750",
            lineHeight: "1.14",
            letterSpacing: "0.01em",
            paddingBottom: "0.06em",
            borderBottom: "1px solid color-mix(in srgb, var(--heading-accent) 52%, transparent)",
          },
          ".unified-rendered-content h3": {
            "--heading-accent": "var(--accent-secondary)",
            color: "var(--heading-accent)",
            fontSize: "1.25em",
            fontWeight: "700",
            lineHeight: "1.25",
            paddingLeft: "0.35em",
            borderLeft: "3px solid color-mix(in srgb, var(--heading-accent) 72%, transparent)",
          },
          ".unified-rendered-content h4, .unified-rendered-content h5, .unified-rendered-content h6": {
            "--heading-accent": "var(--accent-cyan)",
            color: "var(--heading-accent)",
            fontSize: "1.08em",
            fontWeight: "700",
            lineHeight: "1.25",
            margin: "0",
          },
          ".unified-rendered-content a:not(.page-link):not(.tag)": {
            color: "var(--text-link)",
          },
          ".unified-rendered-content .page-link": {
            "--link-accent": "var(--accent-yellow)",
            color: "var(--link-accent)",
            fontFamily: "inherit",
            fontSize: "inherit",
            fontWeight: "inherit",
            lineHeight: "inherit",
            textDecoration: "none",
            border: "1px solid color-mix(in srgb, var(--link-accent) 50%, transparent)",
            borderRadius: "5px",
            background: "color-mix(in srgb, var(--link-accent) 10%, transparent)",
            boxDecorationBreak: "clone",
            WebkitBoxDecorationBreak: "clone",
            padding: "0 0.24em",
          },
          ".unified-rendered-content code": {
            borderRadius: "3px",
            background: "var(--bg-primary)",
            padding: "0 3px",
          },
          ".unified-rendered-content .fc-img": {
            maxWidth: "100%",
            height: "auto",
            borderRadius: "6px",
            margin: "4px 0",
            display: "block",
            cursor: "context-menu",
          },
          ".unified-rendered-content .fc-img[data-src]:not([src])": {
            minHeight: "48px",
            background: "color-mix(in srgb, var(--text-muted) 8%, transparent)",
          },
          ".unified-rendered-content .fc-img[data-image-width], .unified-rendered-content .fc-img[data-image-height]": {
            maxWidth: "none",
          },
        }),
      ],
    });

    editorView = new EditorView({ state, parent: editorHost });
    installVerticalArrowCapture(editorView);
  }

  function activateEditor(view: EditorView) {
    (window as any).__activeEditorView = view;
    (window as any).__unifiedPageEditorView = view;
  }

  function moveVerticalSelection(view: EditorView, direction: "up" | "down", extend: boolean): boolean {
    view.focus();
    const selection = view.state.selection;
    const range = selection.main;
    const moved = view.moveVertically(range, direction === "down");
    const nextRange = extend
      ? EditorSelection.range(
          range.anchor,
          moved.head,
          moved.goalColumn,
          moved.bidiLevel ?? undefined,
          moved.assoc,
        )
      : EditorSelection.cursor(
          moved.head,
          moved.assoc,
          moved.bidiLevel ?? undefined,
          moved.goalColumn,
        );
    const nextSelection = selection.replaceRange(nextRange);
    if (nextSelection.eq(selection, true)) {
      return false;
    }

    view.dispatch({
      selection: nextSelection,
      scrollIntoView: true,
      userEvent: extend ? "select.keyboard" : "move.keyboard",
    });
    requestAnimationFrame(() => view.focus());
    return true;
  }

  function installVerticalArrowCapture(view: EditorView) {
    const onArrowKey = (event: KeyboardEvent) => {
      if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
      if (event.altKey || event.ctrlKey || event.metaKey) return;
      if (!editorView || editorView !== view) return;

      const handled = moveVerticalSelection(
        view,
        event.key === "ArrowUp" ? "up" : "down",
        event.shiftKey,
      );

      if (!handled) return;
      event.preventDefault();
      event.stopImmediatePropagation();
    };

    view.contentDOM.addEventListener("keydown", onArrowKey, true);
    verticalArrowCleanup = () => {
      view.contentDOM.removeEventListener("keydown", onArrowKey, true);
    };
  }

  async function loadSource(pageId: string) {
    const token = ++loadToken;
    loading = true;
    error = null;
    savedMessage = "";

    try {
      const source = await getPageSource(pageId);
      if (token !== loadToken) return;
      await tick();
      if (token !== loadToken) return;
      resetEditor(source);
      dirty = false;
    } catch (e) {
      if (token !== loadToken) return;
      error = `Failed to load page source: ${errorMessage(e)}`;
    } finally {
      if (token === loadToken) {
        loading = false;
      }
    }
  }

  async function saveSource() {
    if (!editorView || saving) return;
    const content = editorView.state.doc.toString();
    saving = true;
    error = null;
    savedMessage = "";

    try {
      await updatePageSource(page.id, content);
      dirty = false;
      savedMessage = "Saved";
      await onReload?.();
    } catch (e) {
      error = `Failed to save page source: ${errorMessage(e)}`;
    } finally {
      saving = false;
    }
  }

  async function reloadFromDisk() {
    if (dirty && !window.confirm("Discard unsaved source edits and reload from disk?")) {
      return;
    }
    await loadSource(page.id);
  }

  function exitPrototype() {
    if (dirty && !window.confirm("Exit the prototype and discard unsaved source edits?")) {
      return;
    }
    onExitPrototype?.();
  }
</script>

<div class="unified-page-editor">
  <div class="prototype-banner">
    <div>
      <strong>Experimental continuous editor</strong>
      <span>One editor surface for this page/day. Block ids stay in source and saves re-index back into block rows.</span>
    </div>
    <div class="prototype-actions">
      {#if savedMessage}
        <span class="save-status">{savedMessage}</span>
      {:else if dirty}
        <span class="dirty-status">Unsaved</span>
      {/if}
      <button type="button" onclick={saveSource} disabled={saving || loading}>
        {saving ? "Saving..." : "Save source"}
      </button>
      <button type="button" onclick={reloadFromDisk} disabled={saving || loading}>Reload</button>
      <button type="button" onclick={exitPrototype}>Classic editor</button>
    </div>
  </div>

  {#if error}
    <div class="prototype-error" role="alert">{error}</div>
  {/if}

  <div class="editor-host" class:loading bind:this={editorHost}></div>

  {#if imageSizeMenu}
    <div
      class="image-size-menu app-context-menu"
      style={`left: ${imageSizeMenu.x}px; top: ${imageSizeMenu.y}px;`}
      role="menu"
      aria-label="Image size menu"
      onpointerdown={(e) => e.stopPropagation()}
      oncontextmenu={(e) => { e.stopPropagation(); e.preventDefault(); }}
      onclick={(e) => e.stopPropagation()}
    >
      <div class="image-size-menu-title">Size</div>
      <button class="image-size-menu-item" type="button" role="menuitem" onclick={resetImageScale}>
        Original Size
      </button>
      {#each IMAGE_SIZE_SCALES as scale}
        <button class="image-size-menu-item" type="button" role="menuitem" onclick={() => chooseImageScale(scale)}>
          {formatImageScale(scale)} <span>{imageScaleSizeLabel(scale)}</span>
        </button>
      {/each}
    </div>
  {/if}

  {#if loading}
    <div class="prototype-loading">Loading source...</div>
  {/if}
</div>

<style>
  .unified-page-editor {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .prototype-banner {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: color-mix(in srgb, var(--accent, #7c3aed) 12%, var(--bg-secondary));
    color: var(--text-secondary);
    font-size: 12px;
  }

  .prototype-banner strong {
    display: block;
    color: var(--text-primary);
    font-size: 12px;
  }

  .prototype-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .prototype-actions button {
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--bg-hover);
    color: var(--text-primary);
    cursor: pointer;
    font-size: 12px;
    padding: 5px 8px;
  }

  .prototype-actions button:disabled {
    cursor: default;
    opacity: 0.55;
  }

  .save-status,
  .dirty-status {
    font-size: 11px;
    color: var(--text-muted);
  }

  .dirty-status {
    color: #ffaa00;
  }

  .prototype-error {
    padding: 8px 10px;
    border: 1px solid rgba(243, 139, 168, 0.35);
    border-radius: 6px;
    background: rgba(243, 139, 168, 0.12);
    color: #f38ba8;
    font-size: 12px;
  }

  .editor-host.loading {
    opacity: 0.6;
  }

  .prototype-loading {
    position: absolute;
    inset: 52px 0 auto 0;
    padding: 12px;
    color: var(--text-muted);
    font-size: 12px;
    pointer-events: none;
  }

  .image-size-menu {
    position: fixed;
    z-index: 2147483000;
    min-width: 212px;
    padding: 6px;
  }

  .image-size-menu-title {
    padding: 5px 8px 6px;
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .image-size-menu-item {
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
  .image-size-menu-item:focus-visible {
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    color: var(--accent);
  }

  @media (max-width: 720px) {
    .prototype-banner {
      align-items: flex-start;
      flex-direction: column;
    }

    .prototype-actions {
      flex-wrap: wrap;
    }
  }
</style>
