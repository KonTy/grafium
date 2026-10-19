<script lang="ts">
  import { autofocus } from "../lib/autofocus";
  import { contextMenuPositionFromEvent } from "../lib/contextMenu";
  import { retainEditorForDialog } from "../lib/editorDialogFocus";
  import { handleMenuKeydown } from "../lib/menuKeyboard";
  import { spellcheck } from "../lib/spellcheck";
  import type { SpellcheckRequest } from "../lib/spellcheckExtension";
  import { describeError, showToast } from "../lib/toast.svelte";

  let { request, onClose }: {
    request: SpellcheckRequest;
    onClose: () => void;
  } = $props();

  let menu = $state<HTMLDivElement>();
  let size = $state({ width: 200, height: 120 });
  let suggestions = $state<string[] | null>(null);

  const position = $derived(contextMenuPositionFromEvent(request, size));

  // The editors close or deactivate when they lose focus unless a dialog has
  // borrowed it, and the menu takes focus as it is created. Hence `pre`: this
  // has to be in place before the first render.
  $effect.pre(() => retainEditorForDialog(request.view.dom));

  $effect(() => {
    let current = true;
    suggestions = null;
    void spellcheck.suggest(request.word).then((found) => {
      if (current) suggestions = found;
    });
    return () => {
      current = false;
    };
  });

  // Suggestions change the menu's height, which decides how far it must move
  // to stay on screen.
  $effect(() => {
    void suggestions;
    if (menu) size = { width: menu.offsetWidth, height: menu.offsetHeight };
  });

  // Captured, because the editors stop some of these events before they reach the window.
  $effect(() => {
    const dismiss = (event: Event) => {
      if (event.target instanceof Node && menu?.contains(event.target)) return;
      onClose();
    };
    window.addEventListener("pointerdown", dismiss, true);
    window.addEventListener("contextmenu", dismiss, true);
    return () => {
      window.removeEventListener("pointerdown", dismiss, true);
      window.removeEventListener("contextmenu", dismiss, true);
    };
  });

  function finish() {
    onClose();
    request.view.focus();
  }

  function replaceWith(replacement: string) {
    const { view, from, to, word } = request;
    finish();
    if (view.state.sliceDoc(from, to) !== word) {
      showToast("The text changed, so the correction was not applied.", "info");
      return;
    }
    view.dispatch({
      changes: { from, to, insert: replacement },
      selection: { anchor: from + replacement.length },
      userEvent: "input.spellcheck",
    });
  }

  async function addToDictionary() {
    const { word } = request;
    finish();
    try {
      await spellcheck.addWord(word);
    } catch (error) {
      showToast(`Could not add "${word}" to the dictionary: ${describeError(error)}`);
    }
  }

  function ignoreForNow() {
    spellcheck.ignore(request.word);
    finish();
  }
</script>

<div
  bind:this={menu}
  class="spell-menu app-context-menu"
  style={`left: ${position.x}px; top: ${position.y}px;`}
  role="menu"
  aria-label="Spelling suggestions"
  tabindex="-1"
  use:autofocus
  onpointerdown={(e) => { e.stopPropagation(); e.preventDefault(); }}
  oncontextmenu={(e) => { e.stopPropagation(); e.preventDefault(); }}
  onclick={(e) => e.stopPropagation()}
  onkeydown={(e) => handleMenuKeydown(e, finish)}
>
  {#if suggestions === null}
    <div class="spell-menu-note" role="menuitem" aria-disabled="true">Looking for suggestions…</div>
  {:else if suggestions.length === 0}
    <div class="spell-menu-note" role="menuitem" aria-disabled="true">No suggestions</div>
  {:else}
    {#each suggestions as suggestion}
      <button
        class="spell-menu-item spell-menu-suggestion"
        type="button"
        role="menuitem"
        onclick={() => replaceWith(suggestion)}
      >
        <span dir="auto">{suggestion}</span>
      </button>
    {/each}
  {/if}
  <div class="spell-menu-separator" role="separator"></div>
  <button class="spell-menu-item" type="button" role="menuitem" onclick={() => void addToDictionary()}>
    Add to dictionary
  </button>
  <button class="spell-menu-item" type="button" role="menuitem" onclick={ignoreForNow}>
    Ignore for now
  </button>
</div>

<style>
  .spell-menu {
    min-width: 170px;
    padding: 6px;
  }

  .spell-menu:focus {
    outline: none;
  }

  .spell-menu-item,
  .spell-menu-note {
    display: flex;
    align-items: center;
    width: 100%;
    padding: 7px 8px;
    border: none;
    border-radius: 6px;
    background: transparent;
    font: inherit;
    font-size: 13px;
    text-align: left;
  }

  .spell-menu-item {
    color: var(--text-primary);
    cursor: pointer;
  }

  .spell-menu-item:hover,
  .spell-menu-item:focus-visible {
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    color: var(--accent);
  }

  .spell-menu-suggestion {
    font-weight: 600;
  }

  .spell-menu-suggestion span {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .spell-menu-note {
    color: var(--text-muted);
    cursor: default;
  }

  .spell-menu-separator {
    height: 1px;
    margin: 5px 2px;
    background: var(--border);
  }
</style>
