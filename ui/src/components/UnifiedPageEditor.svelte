<script lang="ts">
  import { onDestroy, tick } from "svelte";
  import { EditorSelection, EditorState, Prec, RangeSetBuilder } from "@codemirror/state";
  import {
    Decoration,
    type DecorationSet,
    drawSelection,
    EditorView,
    keymap,
    ViewPlugin,
    type ViewUpdate,
    WidgetType,
  } from "@codemirror/view";
  import {
    defaultKeymap,
    history,
    historyKeymap,
  } from "@codemirror/commands";
  import { markdown } from "@codemirror/lang-markdown";
  import type { Page } from "../lib/api";
  import { getPageSource, updatePageSource } from "../lib/api";
  import { hydrateAssetMedia, renderBlock } from "../lib/markdown";

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

  class RenderedBlockWidget extends WidgetType {
    private indent: string;
    private content: string;
    private editPos: number;

    constructor(indent: string, content: string, editPos: number) {
      super();
      this.indent = indent;
      this.content = content;
      this.editPos = editPos;
    }

    eq(other: WidgetType): boolean {
      return other instanceof RenderedBlockWidget
        && other.indent === this.indent
        && other.content === this.content
        && other.editPos === this.editPos;
    }

    toDOM(view: EditorView): HTMLElement {
      const row = document.createElement("div");
      row.className = "unified-rendered-block";
      row.style.setProperty("--preview-depth", String(Math.floor(this.indent.length / 2)));

      const bullet = document.createElement("span");
      bullet.className = "unified-rendered-bullet";
      bullet.textContent = "•";

      const content = document.createElement("div");
      content.className = "unified-rendered-content rendered-content";
      if (this.content.trim()) {
        content.innerHTML = renderBlock(this.content);
      } else {
        content.appendChild(document.createTextNode("\u00a0"));
      }

      row.append(bullet, content);
      row.addEventListener("mousedown", (event) => {
        if (event.button !== 0) return;
        view.dispatch({
          selection: EditorSelection.cursor(this.editPos),
          scrollIntoView: true,
          userEvent: "select.pointer",
        });
        view.focus();
      });
      queueMicrotask(() => void hydrateAssetMedia(content));
      return row;
    }

    ignoreEvent(): boolean {
      return false;
    }

    get estimatedHeight(): number {
      return 26;
    }
  }

  function isHiddenPropertyLine(text: string): boolean {
    return /^\s*id::\s+\S+\s*$/.test(text);
  }

  function buildBlockPreviewDecorations(view: EditorView): DecorationSet {
    const builder = new RangeSetBuilder<Decoration>();
    const activeLine = view.state.doc.lineAt(view.state.selection.main.head).number;

    for (let lineNo = 1; lineNo <= view.state.doc.lines; lineNo++) {
      const line = view.state.doc.line(lineNo);
      const text = line.text;

      if (isHiddenPropertyLine(text)) {
        builder.add(line.from, line.from, Decoration.line({ class: "cm-hidden-id-line" }));
        continue;
      }

      if (lineNo === activeLine) continue;

      const blockMatch = text.match(/^(\s*)-\s?(.*)$/);
      if (!blockMatch) continue;

      const indent = blockMatch[1] ?? "";
      const content = blockMatch[2] ?? "";
      const contentOffset = indent.length + (text[indent.length + 1] === " " ? 2 : 1);
      builder.add(
        line.from,
        line.to,
        Decoration.replace({
          widget: new RenderedBlockWidget(indent, content, line.from + contentOffset),
          inclusive: false,
        }),
      );
    }

    return builder.finish();
  }

  const blockPreviewPlugin = ViewPlugin.fromClass(class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = buildBlockPreviewDecorations(view);
    }

    update(update: ViewUpdate) {
      if (update.docChanged || update.selectionSet || update.viewportChanged) {
        this.decorations = buildBlockPreviewDecorations(update.view);
      }
    }
  }, {
    decorations: (plugin) => plugin.decorations,
  });

  $effect(() => {
    const pageId = page.id;
    void loadSource(pageId);
  });

  onDestroy(() => {
    destroyEditor();
  });

  function errorMessage(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
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
        history(),
        drawSelection(),
        blockPreviewPlugin,
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
          },
          blur: (_event, view) => {
            if ((window as any).__activeEditorView === view) {
              (window as any).__activeEditorView = undefined;
            }
          },
        }),
        EditorView.theme({
          "&": {
            fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', sans-serif",
            fontSize: compact ? "13px" : "14px",
            lineHeight: "1.45",
            color: "var(--text-primary)",
            background: "transparent",
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
            fontFamily: "'JetBrains Mono', 'Fira Code', ui-monospace, monospace",
            overflow: "visible",
            maxHeight: "none",
          },
          ".cm-content": {
            padding: "10px 12px",
            caretColor: "var(--text-primary)",
            minHeight: compact ? "120px" : "240px",
          },
          ".cm-line": {
            padding: "0 4px",
          },
          ".cm-hidden-id-line": {
            display: "none !important",
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
          ".unified-rendered-block": {
            display: "grid",
            gridTemplateColumns: "16px minmax(0, 1fr)",
            columnGap: "4px",
            alignItems: "baseline",
            paddingLeft: "calc(var(--preview-depth, 0) * 20px)",
            color: "var(--text-primary)",
            fontFamily: "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', sans-serif",
            whiteSpace: "normal",
          },
          ".unified-rendered-bullet": {
            color: "var(--text-muted)",
            userSelect: "none",
          },
          ".unified-rendered-content": {
            minWidth: "0",
          },
          ".unified-rendered-content > :first-child": {
            marginTop: "0",
          },
          ".unified-rendered-content > :last-child": {
            marginBottom: "0",
          },
          ".unified-rendered-content h1, .unified-rendered-content h2, .unified-rendered-content h3": {
            lineHeight: "1.2",
            margin: "0",
          },
          ".unified-rendered-content a": {
            color: "var(--accent)",
          },
          ".unified-rendered-content code": {
            borderRadius: "3px",
            background: "var(--bg-primary)",
            padding: "0 3px",
          },
        }),
      ],
    });

    editorView = new EditorView({ state, parent: editorHost });
    activateEditor(editorView);
    editorView.focus();
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
      <strong>Unified editor prototype</strong>
      <span>One CodeMirror surface for this page source, then re-indexed back into block rows.</span>
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
