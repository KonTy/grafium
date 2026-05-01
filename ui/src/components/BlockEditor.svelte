<script lang="ts">
  import { onMount } from "svelte";
  import { EditorView, keymap, placeholder as cmPlaceholder, lineNumbers } from "@codemirror/view";
  import { EditorState, EditorSelection } from "@codemirror/state";
  import { defaultKeymap, indentWithTab, history, historyKeymap, undo, redo } from "@codemirror/commands";
  import { markdown } from "@codemirror/lang-markdown";
  import { renderBlock } from "../lib/markdown";
  import { updateBlock, createBlock, deleteBlock } from "../lib/api";
  import { keymap_manager } from "../lib/keymap";
  import { htmlToMarkdown, splitMarkdownIntoBlocks } from "../lib/htmlToMd";
  import type { PasteBlock } from "../lib/htmlToMd";
  import type { Block } from "../lib/api";

  interface Props {
    block: Block;
    pageId: string;
    depth?: number;
    focused?: boolean;
    selected?: boolean;
    onFocus?: (blockId: string) => void;
    onBlur?: (blockId: string) => void;
    onEnter?: (blockId: string, content: string, orderIndex: number) => void;
    onDelete?: (blockId: string) => void;
    onIndent?: (blockId: string, direction: "in" | "out") => void;
    onNavigate?: (blockId: string, direction: "up" | "down") => void;
    onBulletClick?: (blockId: string, event: MouseEvent) => void;
    onPasteBlocks?: (blockId: string, blocks: PasteBlock[]) => void;
  }

  let {
    block,
    pageId,
    depth = 0,
    focused = false,
    selected = false,
    onFocus,
    onBlur,
    onEnter,
    onDelete,
    onIndent,
    onNavigate,
    onBulletClick,
    onPasteBlocks,
  }: Props = $props();

  let editorContainer: HTMLDivElement;
  let editorView: EditorView | undefined;
  let savedState: EditorState | undefined;
  let shiftHeld = false;
  let isEditing = $state(false);
  let isCodeBlock = $derived(detectCodeBlock(block.content));
  let renderedHtml = $derived(renderBlock(block.content));

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

  // Save content on blur
  async function saveContent(content: string) {
    if (content !== block.content) {
      await updateBlock(block.id, content);
      block.content = content;
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

  function startEditing() {
    isEditing = true;
    keymap_manager.isEditing = true;
    onFocus?.(block.id);

    // Wait for DOM update then create editor
    requestAnimationFrame(() => {
      if (!editorContainer) return;

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
          keymap.of([
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
                const content = view.state.doc.toString();
                saveContent(content);
                onEnter?.(block.id, content, block.order_index);
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
                onIndent?.(block.id, "in");
                return true;
              },
            },
            {
              key: "Shift-Tab",
              run: (view) => {
                if (isInsideCodeFence(view)) {
                  return false; // let default handle dedent
                }
                onIndent?.(block.id, "out");
                return true;
              },
            },
            {
              key: "ArrowUp",
              run: (view) => {
                // Inside code fence with lines above: move within
                if (isInsideCodeFence(view)) {
                  const { head } = view.state.selection.main;
                  const line = view.state.doc.lineAt(head);
                  if (line.number > 1) return false; // let default handle
                }
                onNavigate?.(block.id, "up");
                return true;
              },
            },
            {
              key: "ArrowDown",
              run: (view) => {
                // Inside code fence with lines below: move within
                if (isInsideCodeFence(view)) {
                  const { head } = view.state.selection.main;
                  const line = view.state.doc.lineAt(head);
                  if (line.number < view.state.doc.lines) return false;
                }
                onNavigate?.(block.id, "down");
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
          EditorView.theme({
            "&": {
              fontSize: "15px",
              fontFamily: "inherit",
            },
            ".cm-content": {
              padding: "0",
              caretColor: "var(--text-primary)",
            },
            "&.cm-focused": {
              outline: "none",
            },
            ".cm-line": {
              padding: "0",
            },
          }),
          EditorView.domEventHandlers({
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
              }
              return true;
            },
            blur: (_, view) => {
              const content = view.state.doc.toString();
              // Update block.content synchronously BEFORE onBlur fires,
              // so handleBlur sees the current text (not stale empty content)
              block.content = content;
              saveContent(content);
              savedState = view.state;
              (window as any).__activeEditorView = undefined;
              editorView?.destroy();
              editorView = undefined;
              isEditing = false;
              keymap_manager.isEditing = false;
              onBlur?.(block.id);
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
    });
  }

  function stopEditing() {
    if (editorView) {
      const content = editorView.state.doc.toString();
      saveContent(content);
      editorView.destroy();
      editorView = undefined;
    }
    isEditing = false;
    keymap_manager.isEditing = false;
  }

  // Clean up
  $effect(() => {
    return () => {
      if (editorView) {
        editorView.destroy();
      }
    };
  });

  function handleClick() {
    if (!isEditing) {
      startEditing();
    }
  }

  function handleRenderedClick(e: MouseEvent) {
    const target = e.target as HTMLElement;

    // Handle page link clicks
    if (target.classList.contains("page-link")) {
      e.stopPropagation();
      const pageName = target.dataset.page;
      if (pageName) {
        window.dispatchEvent(new CustomEvent("navigate-page", { detail: pageName }));
      }
      return;
    }

    // Handle tag clicks
    if (target.classList.contains("tag")) {
      e.stopPropagation();
      const tag = target.dataset.tag;
      if (tag) {
        window.dispatchEvent(new CustomEvent("navigate-page", { detail: tag }));
      }
      return;
    }

    startEditing();
  }
</script>

<div
  class="block-item"
  class:editing={isEditing}
  class:selected
  class:code-block={isCodeBlock !== null}
  style="padding-left: {depth * 24}px"
>
  {#if !block.content.trim().startsWith("```") && block.content.trim() !== ""}
    <div class="bullet-container" onclick={(e) => { e.stopPropagation(); onBulletClick?.(block.id, e); }}>
      <span class="bullet">•</span>
    </div>
  {/if}
  <div class="block-content" onclick={handleClick}>
    {#if isEditing}
      <div class="editor-wrapper" bind:this={editorContainer}></div>
    {:else}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="rendered-content" onclick={handleRenderedClick}>
        {#if block.content.trim() === ""}
          <span class="placeholder">&nbsp;</span>
        {:else}
          {@html renderedHtml}
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .block-item {
    display: flex;
    align-items: flex-start;
    min-height: 28px;
    border-radius: 4px;
    transition: background-color 0.1s;
    scroll-margin: 40px;
  }

  .block-item:hover {
    background: var(--bg-hover);
  }

  .block-item.editing {
    background: var(--bg-active);
  }

  .block-item.selected {
    background: var(--accent, #7c3aed);
    background: color-mix(in srgb, var(--accent, #7c3aed) 20%, transparent);
  }

  .bullet-container {
    width: 20px;
    min-height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    cursor: pointer;
  }

  .block-item:has(.rendered-content > :first-child:is(h1)) .bullet-container {
    min-height: calc(1.8em * 1.6);
  }

  .block-item:has(.rendered-content > :first-child:is(h2)) .bullet-container {
    min-height: calc(1.5em * 1.6);
  }

  .block-item:has(.rendered-content > :first-child:is(h3)) .bullet-container {
    min-height: calc(1.25em * 1.6);
  }

  .block-item:has(.rendered-content > :first-child:is(h4, h5, h6)) .bullet-container {
    min-height: calc(1.1em * 1.6);
  }

  .bullet {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-secondary);
    display: block;
    font-size: 0;
  }

  .block-content {
    flex: 1;
    min-height: 28px;
    display: flex;
    align-items: flex-start;
    cursor: text;
    line-height: 1.6;
  }

  .editor-wrapper {
    width: 100%;
  }

  .rendered-content {
    width: 100%;
    padding: 2px 0;
  }

  .placeholder {
    color: var(--text-muted);
    font-style: italic;
  }

  .rendered-content :global(.page-link) {
    color: var(--accent);
    cursor: pointer;
    text-decoration: none;
    border-bottom: 1px solid transparent;
  }

  .rendered-content :global(.page-link:hover) {
    border-bottom-color: var(--accent);
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
    color: var(--accent);
    text-decoration: underline;
  }

  .rendered-content :global(h1) {
    font-size: 1.8em;
    font-weight: 700;
    margin: 0;
  }

  .rendered-content :global(h2) {
    font-size: 1.5em;
    font-weight: 600;
    margin: 0;
  }

  .rendered-content :global(h3) {
    font-size: 1.25em;
    font-weight: 600;
    margin: 0;
  }

  .rendered-content :global(h4),
  .rendered-content :global(h5),
  .rendered-content :global(h6) {
    font-size: 1.1em;
    font-weight: 600;
    margin: 0;
  }

  .rendered-content :global(blockquote) {
    border-left: 3px solid var(--accent);
    padding-left: 12px;
    margin: 4px 0;
    color: var(--text-secondary);
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

  .rendered-content :global(.code-block-inner) {
    display: flex;
    overflow-x: auto;
  }

  .rendered-content :global(.line-numbers) {
    display: flex;
    flex-direction: column;
    padding: 10px 0;
    min-width: 32px;
    text-align: right;
    user-select: none;
    border-right: 1px solid var(--border);
  }

  .rendered-content :global(.line-number) {
    font-family: 'JetBrains Mono', 'Fira Code', monospace;
    font-size: 12px;
    line-height: 1.5;
    padding: 0 8px;
    color: var(--text-muted);
  }

  .rendered-content :global(.code-block-pre) {
    margin: 0;
    padding: 10px 12px;
    background: none;
    border-radius: 0;
    flex: 1;
    overflow-x: auto;
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
  }

  .rendered-content :global(ul),
  .rendered-content :global(ol) {
    margin: 2px 0;
    padding-left: 20px;
  }

  .rendered-content :global(li) {
    margin: 2px 0;
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
</style>
