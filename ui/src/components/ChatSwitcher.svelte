<script lang="ts">
  import {
    assistantConversationChanges, assistantConversationRunning,
    createAssistantConversation, deleteAssistantConversation,
    listAssistantConversations, renameAssistantConversation,
    type AssistantThread,
  } from "../lib/assistantConversations";
  import { contextMenuPositionFromEvent } from "../lib/contextMenu";
  import {
    deletionPrompt, deletionTakesEverything, deletionTargets,
    pruneSelection, selectRange,
  } from "../lib/chatSelection";
  import { SvelteSet } from "svelte/reactivity";

  let { graphPath, currentId, onSelect }: {
    graphPath: string;
    currentId: string | null;
    onSelect: (thread: AssistantThread) => void;
  } = $props();

  let renamingId = $state<string | null>(null);
  let renameDraft = $state("");
  let menu = $state<{ x: number; y: number; thread: AssistantThread } | null>(null);
  const selected = new SvelteSet<string>();
  // Where a shift-click measures from. Reset whenever the selection is
  // cleared, so the next shift-click starts a fresh range instead of
  // reaching back to a row the user has since forgotten about.
  let anchorId = $state<string | null>(null);

  // Any click anywhere else dismisses the menu, including the right-click that
  // opens a different one.
  $effect(() => {
    function close(): void { menu = null; }
    window.addEventListener("click", close);
    window.addEventListener("contextmenu", close);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("contextmenu", close);
    };
  });

  // Re-read on every conversation change: the list is derived state, and the
  // store signals rather than exposing a reactive collection.
  let threads = $derived.by(() => {
    $assistantConversationChanges;
    return graphPath ? listAssistantConversations(graphPath) : [];
  });

  // A chat can disappear from under the selection: deleted from its own row,
  // or the whole list swapped when the graph changed. Left alone those ids
  // would make the button's count overstate what it is about to delete.
  $effect(() => {
    const live = threads.map((thread) => thread.id);
    const kept = pruneSelection(selected, live);
    if (kept.size === selected.size) return;
    selected.clear();
    for (const id of kept) selected.add(id);
    if (anchorId !== null && !kept.has(anchorId)) anchorId = null;
  });

  let targets = $derived(deletionTargets(threads, selected));
  let takesEverything = $derived(deletionTakesEverything(threads.length, targets.length));
  let runningTargets = $derived(targets.filter((thread) => assistantConversationRunning(thread)).length);
  let deleteHint = $derived(
    takesEverything
      ? threads.length === 1 ? "Delete this chat" : `Delete all ${threads.length} chats`
      : `Delete ${targets.length} selected ${targets.length === 1 ? "chat" : "chats"}`,
  );

  // Escape is the way out of a selection you did not mean to start.
  $effect(() => {
    function onKey(event: KeyboardEvent): void {
      if (event.key !== "Escape" || selected.size === 0) return;
      if (renamingId !== null) return; // the rename box owns Escape while it is open
      clearSelection();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  function clearSelection(): void {
    selected.clear();
    anchorId = null;
  }

  function toggle(event: MouseEvent, thread: AssistantThread): void {
    event.stopPropagation();
    // Let `checked` be the single source of truth. The native toggle would
    // otherwise flip the DOM under a shift-click that leaves the row selected,
    // and Svelte would not correct it because the expression never changed.
    event.preventDefault();
    if (event.shiftKey) {
      // A range always selects; using it to deselect would make a single
      // shift-click wipe a carefully built selection.
      for (const id of selectRange(threads.map((t) => t.id), anchorId, thread.id)) {
        selected.add(id);
      }
    } else if (selected.has(thread.id)) {
      selected.delete(thread.id);
    } else {
      selected.add(thread.id);
    }
    anchorId = thread.id;
    if (selected.size === 0) anchorId = null;
  }

  function label(thread: AssistantThread): string {
    if (thread.title.trim()) return thread.title;
    if (thread.sourcePageId) return thread.sourcePageTitle || "Page chat";
    return "New chat";
  }

  function startRename(thread: AssistantThread): void {
    renamingId = thread.id;
    renameDraft = label(thread);
  }

  async function commitRename(thread: AssistantThread): Promise<void> {
    const title = renameDraft;
    renamingId = null;
    await renameAssistantConversation(thread, title);
  }

  function onRenameKey(event: KeyboardEvent, thread: AssistantThread): void {
    if (event.key === "Enter") {
      event.preventDefault();
      void commitRename(thread);
    } else if (event.key === "Escape") {
      event.preventDefault();
      // The window listener clears the selection on Escape, and it would
      // otherwise see this one too -- by then renamingId is already null, so
      // guarding on it there is not enough.
      event.stopPropagation();
      renamingId = null;
    }
  }

  function openMenu(event: MouseEvent, thread: AssistantThread): void {
    event.preventDefault();
    // The window listener that closes menus also sees this event, so let it
    // run first and open afterwards -- otherwise the menu closes itself.
    const at = contextMenuPositionFromEvent(event, { width: 160, height: 80 });
    queueMicrotask(() => { menu = { ...at, thread }; });
  }

  async function remove(thread: AssistantThread): Promise<void> {
    await deleteAssistantConversation(thread);
    reselectIfCurrentIsGone();
  }

  /**
   * One button, two jobs: with nothing selected it takes the whole list, and
   * with a selection it takes only that. The confirmation names which, because
   * the difference lives in the selection state rather than on the button.
   */
  async function removeTargets(): Promise<void> {
    // Snapshot first: deleting republishes the list, so the derived array
    // would shrink underneath the loop.
    const doomed = [...targets];
    if (!doomed.length) return;
    const message = deletionPrompt(doomed.length, takesEverything, runningTargets);
    if (!window.confirm(message)) return;
    clearSelection();
    for (const thread of doomed) await deleteAssistantConversation(thread);
    reselectIfCurrentIsGone();
  }

  /** The panel always needs an open conversation, even after deleting them all. */
  function reselectIfCurrentIsGone(): void {
    const remaining = listAssistantConversations(graphPath);
    if (remaining.some((thread) => thread.id === currentId)) return;
    onSelect(remaining[0] ?? createAssistantConversation(graphPath));
  }
</script>

<div class="switcher" class:selecting={selected.size > 0}>
  <div class="switcher-head">
    <h2>{selected.size > 0 ? `${selected.size} selected` : "Chats"}</h2>
    <button
      type="button"
      class="delete-chats"
      onclick={() => void removeTargets()}
      disabled={!threads.length}
      title={threads.length ? deleteHint : "No conversations to delete"}
      aria-label={threads.length ? deleteHint : "No conversations to delete"}
    >
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
        <path d="M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14" />
      </svg>
    </button>
    <button
      type="button"
      class="new-chat"
      onclick={() => onSelect(createAssistantConversation(graphPath))}
      disabled={!graphPath}
      title="Start another conversation"
    >New chat</button>
  </div>
  <ul>
    {#each threads as thread (thread.id)}
      <li
        class:current={thread.id === currentId}
        class:picked={selected.has(thread.id)}
        oncontextmenu={(event) => openMenu(event, thread)}
      >
        {#if renamingId === thread.id}
          <!-- svelte-ignore a11y_autofocus -->
          <input
            class="rename"
            autofocus
            bind:value={renameDraft}
            onkeydown={(event) => onRenameKey(event, thread)}
            onblur={() => void commitRename(thread)}
            aria-label="Conversation name"
          />
        {:else}
          <input
            type="checkbox"
            class="select"
            checked={selected.has(thread.id)}
            onclick={(event) => toggle(event, thread)}
            title="Select for deletion. Shift-click to select a range."
            aria-label={`Select ${label(thread)}`}
          />
          <button type="button" class="pick" onclick={() => onSelect(thread)} title={label(thread)}>
            <span class="name">{label(thread)}</span>
            {#if assistantConversationRunning(thread)}
              <span class="running" aria-label="This conversation is working">●</span>
            {:else if thread.queuePosition > 0}
              <span class="queued" aria-label="Waiting for the model">{thread.queuePosition}</span>
            {/if}
          </button>
          <span class="actions">
            <button type="button" onclick={() => startRename(thread)} title="Rename">✎</button>
            <button type="button" onclick={() => void remove(thread)} title="Delete">✕</button>
          </span>
        {/if}
      </li>
    {/each}
    {#if !threads.length}
      <li class="empty">No conversations yet.</li>
    {/if}
  </ul>
</div>

{#if menu}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    class="context-menu app-context-menu"
    style="top:{menu.y}px;left:{menu.x}px;"
    onclick={(event) => event.stopPropagation()}
  >
    <button type="button" class="context-menu-item" onclick={() => { const t = menu!.thread; menu = null; startRename(t); }}>Rename</button>
    <button type="button" class="context-menu-item" onclick={() => { const t = menu!.thread; menu = null; void remove(t); }}>Delete</button>
  </div>
{/if}

<style>
  .switcher { display: flex; flex-direction: column; min-height: 0; width: 200px; flex: 0 0 auto; border-right: 1px solid var(--border-color, #ddd); padding-right: 10px; margin-right: 12px; }
  .switcher-head { display: flex; align-items: center; gap: 6px; margin-bottom: 6px; }
  h2 { margin: 0; font-size: 12px; text-transform: uppercase; letter-spacing: 0.04em; color: var(--text-secondary, #666); }
  .delete-chats { display: inline-flex; align-items: center; justify-content: center; margin-left: auto; padding: 3px; border: 1px solid transparent; border-radius: 4px; background: none; color: var(--text-secondary, #888); cursor: pointer; }
  .delete-chats:hover:not(:disabled) { color: var(--danger-color, #c0392b); border-color: var(--border-color, #ddd); }
  .delete-chats:disabled { opacity: 0.4; cursor: default; }
  .new-chat { font-size: 11px; padding: 3px 7px; border: 1px solid var(--border-color, #ddd); border-radius: 4px; background: var(--bg-secondary, #f5f5f5); color: var(--text-primary); cursor: pointer; }
  .new-chat:disabled { opacity: 0.5; cursor: default; }
  ul { list-style: none; margin: 0; padding: 0; overflow-y: auto; min-height: 0; }
  li { display: flex; align-items: center; gap: 2px; border-radius: 4px; }
  li.current { background: var(--bg-secondary, #eef); }
  li.picked { box-shadow: inset 2px 0 0 var(--accent-color, #4a90d9); }
  li.empty { padding: 6px 4px; font-size: 12px; color: var(--text-secondary, #888); }
  .pick { flex: 1; min-width: 0; display: flex; align-items: center; gap: 6px; text-align: left; background: none; border: 0; padding: 6px 4px; font-size: 13px; color: var(--text-primary); cursor: pointer; }
  .name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .running { color: var(--accent-color, #4a90d9); font-size: 10px; }
  .queued { font-size: 10px; color: var(--text-secondary, #888); border: 1px solid currentColor; border-radius: 8px; padding: 0 4px; }
  .actions { display: none; gap: 2px; }
  li:hover .actions, li.current .actions { display: flex; }
  /* Hidden until wanted, so a 200px list is not permanently crowded; once
     anything is selected every box stays visible, because a selection you
     cannot see is the whole risk of one button doing two jobs. */
  .select { flex: 0 0 auto; width: 13px; height: 13px; margin: 0 0 0 4px; accent-color: var(--accent-color, #4a90d9); cursor: pointer; visibility: hidden; }
  li:hover .select, .selecting .select { visibility: visible; }
  .actions button { background: none; border: 0; padding: 2px 4px; color: var(--text-secondary, #888); cursor: pointer; font-size: 12px; }
  .actions button:hover { color: var(--text-primary); }
  .rename { flex: 1; min-width: 0; margin: 3px 2px; padding: 3px 5px; font-size: 13px; border: 1px solid var(--accent-color, #4a90d9); border-radius: 4px; background: var(--bg-primary); color: var(--text-primary); }
  .context-menu { position: fixed; z-index: 2147483000; border-radius: 6px; padding: 4px; min-width: 150px; }
  .context-menu-item { display: flex; align-items: center; width: 100%; padding: 7px 10px; background: none; border: none; border-radius: 4px; color: var(--text-secondary); font-size: 13px; cursor: pointer; text-align: left; }
  .context-menu-item:hover { background: var(--bg-hover); color: var(--text-primary); }
  @media (max-width: 640px) { .switcher { width: 132px; } }
</style>
