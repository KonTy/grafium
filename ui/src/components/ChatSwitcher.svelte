<script lang="ts">
  import {
    assistantConversationChanges, assistantConversationRunning,
    createAssistantConversation, deleteAssistantConversation,
    listAssistantConversations, renameAssistantConversation,
    type AssistantThread,
  } from "../lib/assistantConversations";

  let { graphPath, currentId, onSelect }: {
    graphPath: string;
    currentId: string | null;
    onSelect: (thread: AssistantThread) => void;
  } = $props();

  let renamingId = $state<string | null>(null);
  let renameDraft = $state("");

  // Re-read on every conversation change: the list is derived state, and the
  // store signals rather than exposing a reactive collection.
  let threads = $derived.by(() => {
    $assistantConversationChanges;
    return graphPath ? listAssistantConversations(graphPath) : [];
  });

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
      renamingId = null;
    }
  }

  async function remove(thread: AssistantThread): Promise<void> {
    await deleteAssistantConversation(thread);
    if (currentId === thread.id) {
      const remaining = listAssistantConversations(graphPath);
      onSelect(remaining[0] ?? createAssistantConversation(graphPath));
    }
  }
</script>

<div class="switcher">
  <div class="switcher-head">
    <h2>Chats</h2>
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
      <li class:current={thread.id === currentId}>
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

<style>
  .switcher { display: flex; flex-direction: column; min-height: 0; width: 200px; flex: 0 0 auto; border-right: 1px solid var(--border-color, #ddd); padding-right: 10px; margin-right: 12px; }
  .switcher-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-bottom: 6px; }
  h2 { margin: 0; font-size: 12px; text-transform: uppercase; letter-spacing: 0.04em; color: var(--text-secondary, #666); }
  .new-chat { font-size: 11px; padding: 3px 7px; border: 1px solid var(--border-color, #ddd); border-radius: 4px; background: var(--bg-secondary, #f5f5f5); color: var(--text-primary); cursor: pointer; }
  .new-chat:disabled { opacity: 0.5; cursor: default; }
  ul { list-style: none; margin: 0; padding: 0; overflow-y: auto; min-height: 0; }
  li { display: flex; align-items: center; gap: 2px; border-radius: 4px; }
  li.current { background: var(--bg-secondary, #eef); }
  li.empty { padding: 6px 4px; font-size: 12px; color: var(--text-secondary, #888); }
  .pick { flex: 1; min-width: 0; display: flex; align-items: center; gap: 6px; text-align: left; background: none; border: 0; padding: 6px 4px; font-size: 13px; color: var(--text-primary); cursor: pointer; }
  .name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .running { color: var(--accent-color, #4a90d9); font-size: 10px; }
  .queued { font-size: 10px; color: var(--text-secondary, #888); border: 1px solid currentColor; border-radius: 8px; padding: 0 4px; }
  .actions { display: none; gap: 2px; }
  li:hover .actions, li.current .actions { display: flex; }
  .actions button { background: none; border: 0; padding: 2px 4px; color: var(--text-secondary, #888); cursor: pointer; font-size: 12px; }
  .actions button:hover { color: var(--text-primary); }
  .rename { flex: 1; min-width: 0; margin: 3px 2px; padding: 3px 5px; font-size: 13px; border: 1px solid var(--accent-color, #4a90d9); border-radius: 4px; background: var(--bg-primary); color: var(--text-primary); }
  @media (max-width: 640px) { .switcher { width: 132px; } }
</style>
