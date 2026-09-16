<script lang="ts">
  import { editorHasDialogFocus } from "../lib/editorDialogFocus";
  import { readingSelectionCapture, publishContinuousReadingSelection } from "../lib/readingSelection";
  import type { BlockTextSelection } from "../lib/keyboardBlockSelection.svelte";
  import { onDestroy, tick, untrack } from "svelte";
  import { autocompletion, completionStatus } from "@codemirror/autocomplete";
  import {
    EditorSelection,
    EditorState,
    Compartment,
    Prec,
    RangeSetBuilder,
    StateEffect,
    StateField,
    Transaction,
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
  import { loadPageSourceSession, type PageSourceSession } from "../lib/pageSourcePersistence";
  import { editorWriteLockExtension, registerEditorFlush } from "../lib/editorPersistence";
  import { openReadingNoteFromEvent, protectReadingNotePointer } from "../lib/readingNoteLinks";
  import { renderReadingNoteFooter } from "../lib/readingNotes";
  import { touchesReadingNoteDefinition } from "../lib/readingNoteFormat";
  import type { WritingRewrittenDetail } from "../lib/writing";
  import { getLatestCurrentBlockAnchor, setCurrentBlockAnchor } from "../lib/currentBlockAnchor";
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
  import { applyWritingEditorChanges, EDITOR_UNDO_MIN_DEPTH, markStructuralUndoBoundary, undoEditor, redoEditor } from "../lib/editorUndo";
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
    onNavigateBoundary?: (direction: "up" | "down", caretX: number) => void;
    onSelectBoundary?: (blockId: string, direction: "up" | "down", selection: BlockTextSelection, caretX: number, headBlockId?: string) => void;
    selectedBlockIds?: ReadonlySet<string>;
  }

  let { page, compact = false, onReload, onExitPrototype, onNavigateBoundary, onSelectBoundary, selectedBlockIds }: Props = $props();

  let editorHost: HTMLDivElement;
  let editorView: EditorView | undefined;
  let verticalArrowCleanup: (() => void) | undefined;
  let loadToken = 0;
  let sourceReady: Promise<void> = Promise.resolve();
  let sourceSession: PageSourceSession | undefined;
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
    private selected: boolean;
    private noteLabel: string | null;

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
      this.selected = !!block.id && !!selectedBlockIds?.has(block.id);
      this.noteLabel = block.readingNote?.storage === "inline" ? block.readingNote.footnoteLabel : null;
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
        && other.selected === this.selected
        && other.noteLabel === this.noteLabel
        && other.bionic === this.bionic;
    }

    toDOM(view: EditorView): HTMLElement {
      const row = document.createElement("div");
      row.className = "unified-rendered-block";
      row.classList.toggle("selected", this.selected);
      row.style.setProperty("--preview-depth", String(this.depth));
      row.title = "Click to edit this block as raw Markdown";
      if (this.noteLabel) {
        row.dataset.readingNoteFooter = "";
        row.title = "Edit this footnote in Notes";
      }
      if (this.blockId) {
        row.dataset.sourceBlockId = this.blockId;
      }

      const content = document.createElement("div");
      content.className = "unified-rendered-content rendered-content";
      if (this.content.trim()) {
        content.innerHTML = this.noteLabel
          ? renderReadingNoteFooter(this.content, this.noteLabel, this.assetBaseDir)
          : renderBlock(this.content, this.assetBaseDir);
      } else {
        content.appendChild(document.createTextNode("\u00a0"));
      }

      if (this.noteLabel || isFencedCodeBlock(this.content)) {
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
        if (openReadingNoteFromEvent(event, page.id, this.blockId)) return;
        if (this.noteLabel) { event.stopPropagation(); return; }
        if (row.ownerDocument.getSelection()?.toString()) return;
        const link = event.target instanceof Element
          ? event.target.closest<HTMLElement>(".page-link[data-page], .tag[data-tag]")
          : null;
        const pageName = link?.dataset.page ?? link?.dataset.tag;
        if (pageName) {
          event.preventDefault();
          event.stopPropagation();
          window.dispatchEvent(new CustomEvent("navigate-page", { detail: { pageName } }));
          return;
        }
        row.ownerDocument.getSelection()?.removeAllRanges();
        event.preventDefault();
        event.stopPropagation();
        view.dispatch({
          selection: EditorSelection.cursor(this.editPos),
          effects: activeBlockEffects(this.blockKey),
          scrollIntoView: true,
          userEvent: "select.pointer",
        });
        row.addEventListener("pointerdown", protectReadingNotePointer);
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
    if (block.readingNote?.storage === "inline") {
      return block.previewTo < sourceMap.source.length ? block.previewTo + 1 : block.previewTo;
    }
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
    for (const hidden of sourceMap.hiddenRanges ?? []) {
      if (hidden.to > hidden.from) ranges.push({ ...hidden, decoration: Decoration.replace({ block: true }) });
    }

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
      if (activeEditing && block === activeBlock && block.readingNote?.storage !== "inline") {
        continue;
      }
      ranges.push({
        from: block.previewFrom,
        to: blockPreviewWithBreakTo(sourceMap, block),
        decoration: Decoration.replace({
          widget: new RenderedBlockWidget(block, assetBaseDir),
          block: true,
          // Adjacent block replacements must cover their shared boundary when
          // switching between rendered widgets and source lines.
          inclusive: true,
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
        || transaction.effects.some((effect) => effect.is(bionicModeEffect) || effect.is(blockSelectionEffect))
        ? buildBlockPreviewDecorations(transaction.state)
        : decorations;
    },
    provide(field) {
      return EditorView.decorations.from(field);
    },
  });

  const blockSelectionEffect = StateEffect.define<void>();
  $effect(() => {
    void selectedBlockIds;
    editorView?.dispatch({ effects: blockSelectionEffect.of(undefined) });
  });

  $effect(() => {
    const pageId = page.id;
    sourceReady = untrack(() => loadSource(pageId));
  });

  $effect(() => bionicReaderEnabled.subscribe(() => {
    editorView?.dispatch({ effects: bionicModeEffect.of(undefined) });
  }));

  onDestroy(() => {
    loadToken += 1;
    sourceSession?.invalidate();
    setCurrentBlockAnchor(page.id, null);
    destroyEditor();
  });

  $effect(() => registerEditorFlush(page.id, async () => {
    await sourceReady;
    if (dirty || saving) await saveSource();
    if (dirty || error) throw new Error(error ?? "Save the page source before continuing.");
  }));

  $effect(() => {
    const handleRewrite = (event: Event) => {
      const detail = (event as CustomEvent<WritingRewrittenDetail>).detail;
      if (detail?.pageId !== page.id) return;
      sourceReady = loadSource(page.id, false, detail);
      detail.refreshes.push(sourceReady.then(() => { if (error) throw new Error(error); }));
    };
    window.addEventListener("grafium-writing-rewritten", handleRewrite);
    return () => window.removeEventListener("grafium-writing-rewritten", handleRewrite);
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
        editorWriteLockExtension(page.id),
        history({ minDepth: EDITOR_UNDO_MIN_DEPTH }),
        EditorState.transactionFilter.of((transaction) => {
          if (!transaction.docChanged || transaction.annotation(Transaction.addToHistory) === false) return transaction;
          const changes: { from: number; to: number }[] = [];
          transaction.changes.iterChangedRanges((from, to) => changes.push({ from, to }));
          if (!touchesReadingNoteDefinition(transaction.startState.doc.toString(), changes)) return transaction;
          queueMicrotask(() => { savedMessage = "Edit managed footnotes in the Notes tab."; });
          return [];
        }),
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
          { key: "Mod-z", run: undoEditor },
          { key: "Mod-Shift-z", run: redoEditor },
          { key: "Mod-y", run: redoEditor },
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
          if (update.view.hasFocus && (update.selectionSet || update.docChanged)) {
            const { from, to } = update.state.selection.main;
            publishContinuousReadingSelection(update.view.dom, page.id, update.state.doc.toString(), from, to);
          }
          if (update.docChanged) {
            if (sourceSession) sourceSession.content = update.state.doc.toString();
            dirty = sourceSession?.dirty ?? true;
            savedMessage = "";
          }
          if (update.view.hasFocus && (update.docChanged || update.selectionSet || update.focusChanged)) {
            publishBlockAnchor(update.view);
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
            if (editorHasDialogFocus(view.dom)) return;
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
            // Widget spacing must be included in CodeMirror's height measurement.
            paddingBottom: "1px",
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
          ".unified-rendered-block.selected": {
            background: "color-mix(in srgb, var(--accent, #7c3aed) 20%, transparent)",
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
    publishBlockAnchor(view);
  }

  function publishBlockAnchor(view: EditorView) {
    const blocks = parsePageSourceMap(view.state.doc.toString()).blocks;
    const block = blocks.find((_, index) => view.state.selection.main.head < (blocks[index + 1]?.ownFrom ?? Infinity));
    const current = getLatestCurrentBlockAnchor();
    if (current?.pageId !== page.id || current.blockId !== (block?.id ?? null)) {
      setCurrentBlockAnchor(page.id, block?.id ?? null);
    }
  }

  export async function prepareBlockSelection(isCurrent: () => boolean): Promise<boolean> {
    await sourceReady;
    if (error) throw new Error(error);
    const view = editorView;
    if (!view || !isCurrent()) return false;
    const doc = view.state.doc;
    if (dirty || saving) {
      if (!await saveSource()) throw new Error(error ?? "Save the page source before selecting blocks.");
      if (!isCurrent() || view !== editorView || view.state.doc !== doc) return false;
    }
    if (!isCurrent() || editorView !== view) return false;
    setActiveBlock(view, null);
    return true;
  }

  export async function restoreTextSelection(blockId: string, selection: BlockTextSelection, isCurrent: () => boolean) {
    await sourceReady;
    const view = editorView;
    if (!view || !isCurrent()) return;
    const clamp = (value: number) => Math.max(0, Math.min(view.state.doc.length, value));
    view.dispatch({
      selection: EditorSelection.range(clamp(selection.anchor), clamp(selection.head)),
      effects: activeBlockEffects(blockId),
      scrollIntoView: true,
    });
    view.focus();
  }

  export function revealBlock(blockId: string) {
    const row = editorHost?.querySelector(`[data-source-block-id="${CSS.escape(blockId)}"]`);
    row?.scrollIntoView({ block: "nearest" });
  }

  export async function reloadSource() {
    sourceReady = loadSource(page.id);
    await sourceReady;
    if (error) throw new Error(error);
  }

  export async function focusForNav(x: number | undefined, edge: "top" | "bottom", isCurrent: () => boolean, blockId?: string): Promise<void> {
    await sourceReady;
    if (!isCurrent()) return;
    if (error) throw new Error(error);
    const view = editorView;
    if (!view) throw new Error("Continuous journal editor is not ready");
    const blocks = parsePageSourceMap(view.state.doc.toString()).blocks;
    const block = blockId ? blocks.find((entry) => entry.id === blockId)
      : edge === "top" ? blocks[0] : blocks.at(-1);
    if (blockId && !block) throw new Error("The selected block is no longer available");
    const anchor = edge === "top" ? block?.contentFrom ?? 0 : block?.contentTo ?? view.state.doc.length;
    view.dispatch({
      selection: EditorSelection.cursor(anchor),
      effects: activeBlockEffects(block ? sourceBlockKey(block) : "empty"),
      scrollIntoView: true,
    });
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    if (!isCurrent() || editorView !== view) return;
    const coords = view.coordsAtPos(anchor, edge === "top" ? 1 : -1);
    const pos = x === undefined ? anchor
      : coords && view.posAtCoords({ x, y: edge === "top" ? coords.top + 2 : coords.bottom - 2 });
    const line = view.state.doc.lineAt(anchor);
    const target = Math.max(line.from, Math.min(line.to, pos ?? anchor));
    view.dispatch({ selection: EditorSelection.cursor(target), scrollIntoView: true });
    if (x === undefined) markStructuralUndoBoundary(view);
    view.focus();
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
      if (event.isComposing || event.keyCode === 229) return;
      if (!editorView || editorView !== view) return;
      if (!event.shiftKey && completionStatus(view.state) !== null) return;

      const direction = event.key === "ArrowUp" ? "up" : "down";
      if (event.shiftKey && onSelectBoundary && completionStatus(view.state) === null) {
        const range = view.state.selection.main;
        const blocks = parsePageSourceMap(view.state.doc.toString()).blocks;
        const edgeBlock = direction === "up" ? blocks[0] : blocks.at(-1);
        const boundary = direction === "up" ? edgeBlock?.contentFrom ?? 0
          : edgeBlock?.contentTo ?? view.state.doc.length;
        const caret = view.coordsAtPos(range.head);
        const edgeCaret = view.coordsAtPos(boundary);
        const atBoundary = direction === "up" ? range.head <= boundary : range.head >= boundary;
        if (atBoundary || (caret && edgeCaret && Math.abs(caret.top - edgeCaret.top) <= 1)) {
          // Page-start properties and the final newline belong to the nearest
          // block too, even though they fall outside its own source range.
          const origin = blocks.find((_, index) => range.anchor < (blocks[index + 1]?.ownFrom ?? Infinity));
          if (origin?.id && edgeBlock?.id) {
            event.preventDefault();
            event.stopImmediatePropagation();
            onSelectBoundary(origin.id, direction, { anchor: range.anchor, head: range.head },
              caret?.left ?? 0, edgeBlock.id);
            return;
          }
        }
      }
      if (onNavigateBoundary && !event.shiftKey && view.state.selection.main.empty) {
        const head = view.state.selection.main.head;
        const blocks = parsePageSourceMap(view.state.doc.toString()).blocks;
        const boundary = direction === "up" ? blocks[0]?.contentFrom ?? 0
          : blocks.at(-1)?.contentTo ?? view.state.doc.length;
        const caret = view.coordsAtPos(head);
        const edge = view.coordsAtPos(boundary);
        if (caret && edge && Math.abs(caret.top - edge.top) <= 1) {
          event.preventDefault();
          event.stopImmediatePropagation();
          onNavigateBoundary(direction, caret.left);
          return;
        }
      }

      const handled = moveVerticalSelection(
        view,
        direction,
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

  async function loadSource(pageId: string, discardDraft = false, rewrite?: WritingRewrittenDetail) {
    const previousView = editorView;
    const previousDoc = previousView?.state.doc;
    const previousSession = sourceSession;
    if (saving || (dirty && !discardDraft)) {
      error = "Source refresh stopped to preserve unsaved text. Save it or explicitly reload from disk.";
      return;
    }
    const token = ++loadToken;
    loading = true;
    error = null;
    savedMessage = "";

    try {
      const session = await loadPageSourceSession(pageId,
        previousSession?.pageId === pageId ? previousSession.graphPath : undefined);
      if (token !== loadToken) return;
      await tick();
      if (token !== loadToken) return;
      if (previousView !== editorView || previousView?.state.doc !== previousDoc) {
        throw new Error("Source refresh stopped because you typed while it was loading. Your unsaved text is preserved.");
      }
      if (rewrite && previousView) {
        const blocks = parsePageSourceMap(previousView.state.doc.toString()).blocks;
        const replacements = rewrite.changes.map((change) => {
          const block = blocks.find((block) => block.id === change.blockId);
          return block ? sourceBlockContentReplacement(block, change.afterContent) : null;
        }).filter((replacement) => replacement !== null).sort((a, b) => a.from - b.from);
        const changes = replacements.length === rewrite.changes.length
          && previousView.state.changes(replacements).apply(previousView.state.doc).toString() === session.expectedSource
            ? replacements : { from: 0, to: previousView.state.doc.length, insert: session.expectedSource };
        applyWritingEditorChanges(previousView, changes, rewrite.action, rewrite.undo);
      } else {
        resetEditor(session.expectedSource);
      }
      if (editorView?.state.doc.toString() !== session.expectedSource.replace(/\r\n?/g, "\n")) {
        throw new Error("The editor could not display the verified source. Reload explicitly before saving.");
      }
      previousSession?.invalidate();
      sourceSession = session;
      session.content = editorView?.state.doc.toString() ?? session.expectedSource;
      dirty = session.dirty;
    } catch (e) {
      if (token !== loadToken) return;
      error = `Failed to load page source: ${errorMessage(e)}`;
    } finally {
      if (token === loadToken) {
        loading = false;
      }
    }
  }

  async function saveSource(): Promise<boolean> {
    const view = editorView;
    const session = sourceSession;
    if (!view || !session || loading || session.pageId !== page.id) return false;
    session.content = view.state.doc.toString();
    const write = session.save();
    const stillCurrent = () => editorView === view && sourceSession === session && page.id === session.pageId;
    saving = true;
    error = null;
    savedMessage = "";

    try {
      await write;
      if (!stillCurrent()) return false;
      dirty = session.dirty;
      error = null;
      savedMessage = dirty ? "" : "Saved";
      await onReload?.();
      return true;
    } catch (e) {
      if (stillCurrent()) {
        dirty = session.dirty;
        savedMessage = "";
        error = `Failed to save page source: ${errorMessage(e)}`;
      }
      return false;
    } finally {
      if (stillCurrent()) saving = session.pending > 0;
    }
  }

  async function reloadFromDisk() {
    if (dirty && !window.confirm("Discard unsaved source edits and reload from disk?")) {
      return;
    }
    await loadSource(page.id, true);
  }

  function exitPrototype() {
    if (dirty && !window.confirm("Exit the prototype and discard unsaved source edits?")) {
      return;
    }
    onExitPrototype?.();
  }
</script>

<div class="unified-page-editor" data-page-id={page.id} use:readingSelectionCapture>
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
