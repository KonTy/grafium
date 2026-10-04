<script lang="ts">
  import { tick } from "svelte";
  import SettingsHelp from "./SettingsHelp.svelte";
  import { formatBinding } from "../lib/shortcuts";
  import { bindingFromEvent } from "../lib/keyBinding";
  import {
    addShortcutBinding,
    FIXED_SHORTCUTS,
    isShortcutCustomized,
    onShortcutsChanged,
    removeShortcutBinding,
    resetAllShortcuts,
    resetShortcut,
    SHORTCUT_DEFINITIONS,
    SHORTCUT_SECTIONS,
    shortcutBindings,
    shortcutDefinition,
  } from "../lib/shortcutRegistry";

  /** How long the first key of a sequence like `g j` waits for the second. */
  const SEQUENCE_WAIT_MS = 1500;

  let version = $state(0);
  $effect(() => onShortcutsChanged(() => { version += 1; }));

  let recordingId = $state<string | null>(null);
  let pressed = $state<string[]>([]);
  let sequenceTimer: ReturnType<typeof setTimeout> | undefined;
  let notice = $state<{ id: string; text: string; error?: boolean } | null>(null);
  let conflict = $state<{ id: string; binding: string; usedBy: string } | null>(null);
  let confirmReset = $state(false);
  let root = $state<HTMLElement>();

  /** "A", "A and B", "A, B and C", or "A, B, C and 4 more". */
  function listLabels(labels: string[]): string {
    if (labels.length <= 1) return labels[0] ?? "";
    const shown = labels.length > 3 ? [...labels.slice(0, 3), `${labels.length - 3} more`] : labels;
    return `${shown.slice(0, -1).join(", ")} and ${shown[shown.length - 1]}`;
  }

  // On Linux the window takes Ctrl-Z, Ctrl-Shift-Z and Ctrl-Y before the page
  // sees them; while recording, it hands them here instead of undoing.
  $effect(() => {
    const id = recordingId;
    if (!id) return;
    const record = (binding: string) => {
      if (recordingId !== id) return false;
      recordChord(id, binding);
      return true;
    };
    const host = window as unknown as { __recordNativeShortcut?: (binding: string) => boolean };
    host.__recordNativeShortcut = record;
    return () => {
      if (host.__recordNativeShortcut === record) delete host.__recordNativeShortcut;
    };
  });

  const sections = $derived.by(() => {
    void version;
    return SHORTCUT_SECTIONS.map((section) => ({
      ...section,
      actions: SHORTCUT_DEFINITIONS.filter((definition) => definition.section === section.id)
        .map((definition) => ({
          ...definition,
          bindings: shortcutBindings(definition.id),
          customized: isShortcutCustomized(definition.id),
        })),
      fixed: FIXED_SHORTCUTS.filter((fixed) => fixed.section === section.id),
    }));
  });
  const anyCustomized = $derived.by(() => {
    void version;
    return SHORTCUT_DEFINITIONS.some((definition) => isShortcutCustomized(definition.id));
  });

  async function focusInRow(id: string, selector: string) {
    await tick();
    root?.querySelector<HTMLElement>(`[data-shortcut-id="${id}"] ${selector}`)?.focus();
  }

  function startRecording(id: string) {
    clearTimeout(sequenceTimer);
    recordingId = id;
    pressed = [];
    notice = null;
    conflict = null;
    void focusInRow(id, "[data-shortcut-recorder]");
  }

  function stopRecording(focusAdd = true) {
    const id = recordingId;
    clearTimeout(sequenceTimer);
    recordingId = null;
    pressed = [];
    if (id && focusAdd) void focusInRow(id, ".keymap-add");
  }

  function commit(id: string, binding: string, moveFromConflict = false) {
    const result = addShortcutBinding(id, binding, moveFromConflict);
    stopRecording();
    if (result.ok) {
      notice = null;
      conflict = null;
    } else if ("conflicts" in result) {
      conflict = { id, binding: result.binding, usedBy: listLabels(result.conflicts.map(({ label }) => label)) };
      void focusInRow(id, ".keymap-conflict button");
    } else {
      notice = { id, text: result.error, error: true };
    }
  }

  function recordKey(event: KeyboardEvent, id: string) {
    const plain = !event.ctrlKey && !event.metaKey && !event.altKey;
    if (plain && !event.shiftKey && event.key === "Escape") {
      event.preventDefault();
      stopRecording();
      return;
    }
    // Tab still moves focus, so the recorder never traps the keyboard.
    if (plain && event.key === "Tab") {
      stopRecording(false);
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (event.repeat) return;
    const chord = bindingFromEvent(event);
    if (chord) recordChord(id, chord);
  }

  function recordChord(id: string, chord: string) {
    const definition = shortcutDefinition(id);
    // App-wide shortcuts may be two plain keys in a row, like `g j`.
    if (definition?.scope === "app" && !definition.singlePress && /^[a-z0-9]$/.test(chord)) {
      const next = [...pressed, chord];
      clearTimeout(sequenceTimer);
      if (next.length >= 2) {
        commit(id, next.join(" "));
        return;
      }
      pressed = next;
      sequenceTimer = setTimeout(() => commit(id, next.join(" ")), SEQUENCE_WAIT_MS);
      return;
    }
    commit(id, pressed.length ? [...pressed, chord].join(" ") : chord);
  }

  function remove(id: string, binding: string) {
    removeShortcutBinding(id, binding);
    notice = null;
    conflict = null;
    void focusInRow(id, ".keymap-add");
  }

  function reset(id: string) {
    const { skipped } = resetShortcut(id);
    conflict = null;
    notice = skipped.length
      ? {
          id,
          text: skipped.map(({ binding, usedBy }) =>
            `${formatBinding(binding)} stays with ${listLabels(usedBy.map(({ label }) => label))}.`).join(" ")
            + " Remove it there first to get it back here.",
        }
      : null;
    void focusInRow(id, ".keymap-add");
  }

  function resetAll() {
    resetAllShortcuts();
    confirmReset = false;
    notice = null;
    conflict = null;
  }
</script>

<div class="shortcut-settings" bind:this={root}>
  <div class="shortcut-toolbar">
    <SettingsHelp title="Keyboard shortcuts">
      <p class="section-desc">
        Press <strong>+</strong> beside an action, then the keys you want. Press two letters in a row,
        like <kbd>g j</kbd>, for a sequence; sequences work when you are not editing text.
        Escape cancels. If the keys already do something else, Grafium asks before moving them.
      </p>
      <p class="section-desc">
        Shortcuts are saved on this device and apply to every graph. Keys marked
        <em>built in</em> make typing, undo and menus work, so they cannot be changed.
      </p>
    </SettingsHelp>
    {#if confirmReset}
      <span class="shortcut-confirm" role="group" aria-label="Reset all shortcuts">
        <button type="button" class="danger" onclick={resetAll}>Reset all</button>
        <button type="button" onclick={() => { confirmReset = false; }}>Keep mine</button>
      </span>
    {:else}
      <button type="button" class="reset-all" disabled={!anyCustomized} onclick={() => { confirmReset = true; }}>
        Reset all shortcuts
      </button>
    {/if}
  </div>

  <div class="keymap-list">
    {#each sections as section (section.id)}
      <div class="keymap-category">
        <h3 class="keymap-category-title">{section.title}</h3>
        {#each section.actions as action (action.id)}
          <div class="keymap-row" class:customized={action.customized} data-shortcut-id={action.id}>
            <span class="keymap-desc">{action.label}</span>
            <span class="keymap-keys">
              {#each action.bindings as binding (binding)}
                <span class="keymap-chip">
                  <kbd class="keymap-binding">{formatBinding(binding)}</kbd>
                  <button type="button" class="keymap-remove"
                    aria-label={`Remove ${formatBinding(binding)} from ${action.label}`}
                    title={`Remove ${formatBinding(binding)}`}
                    onclick={() => remove(action.id, binding)}>×</button>
                </span>
              {:else}
                <span class="keymap-none">None</span>
              {/each}
              {#if recordingId === action.id}
                <button type="button" class="keymap-recorder" data-shortcut-recorder
                  aria-label={`Press the new shortcut for ${action.label}. Escape cancels.`}
                  onkeydown={(event) => recordKey(event, action.id)}
                  onblur={() => { if (recordingId === action.id) stopRecording(false); }}>
                  {pressed.length ? `${pressed.join(" ")} …` : "Press keys…"}
                </button>
              {:else}
                <button type="button" class="keymap-add" aria-label={`Add a shortcut for ${action.label}`}
                  title="Add a shortcut" onclick={() => startRecording(action.id)}>+</button>
              {/if}
              {#if action.customized}
                <button type="button" class="keymap-reset" aria-label={`Reset ${action.label} to its default`}
                  title="Reset to default" onclick={() => reset(action.id)}>Reset</button>
              {/if}
            </span>
            {#if notice?.id === action.id}
              <p class="keymap-notice" class:error={notice.error} role={notice.error ? "alert" : "status"}>{notice.text}</p>
            {/if}
            {#if conflict?.id === action.id}
              <div class="keymap-conflict" role="alert">
                <span>{formatBinding(conflict.binding)} is used by {conflict.usedBy}.</span>
                <button type="button" onclick={() => conflict && commit(conflict.id, conflict.binding, true)}>Use here</button>
                <button type="button" onclick={() => { conflict = null; void focusInRow(action.id, ".keymap-add"); }}>Cancel</button>
              </div>
            {/if}
          </div>
        {/each}
        {#each section.fixed as fixed (fixed.label)}
          <div class="keymap-row fixed">
            <span class="keymap-desc">{fixed.label}</span>
            <span class="keymap-keys">
              {#each fixed.keys as binding (binding)}
                <kbd class="keymap-binding">{formatBinding(binding)}</kbd>
              {/each}
              <span class="keymap-fixed">built in</span>
            </span>
          </div>
        {/each}
      </div>
    {/each}
  </div>
</div>

<style>
  .shortcut-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 12px;
  }

  .section-desc kbd {
    font-family: "SF Mono", "Fira Code", "JetBrains Mono", monospace;
    font-size: 11px;
    padding: 2px 5px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 3px;
    color: var(--text-primary);
  }

  .shortcut-confirm {
    display: inline-flex;
    gap: 8px;
  }

  .shortcut-toolbar button,
  .keymap-conflict button,
  .keymap-reset {
    padding: 4px 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-secondary);
    color: var(--text-primary);
    font-size: 12px;
    cursor: pointer;
  }

  .shortcut-toolbar button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .shortcut-toolbar .danger {
    color: var(--accent-red, var(--error, #d1242f));
  }

  .keymap-list {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .keymap-category-title {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-muted);
    margin-bottom: 8px;
  }

  .keymap-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: 6px 12px;
    padding: 6px 0;
  }

  .keymap-row + .keymap-row {
    border-top: 1px solid var(--border);
  }

  .keymap-desc {
    font-size: 13px;
    color: var(--text-secondary);
  }

  .keymap-row.customized .keymap-desc {
    color: var(--text-primary);
  }

  .keymap-keys {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    align-items: center;
    gap: 6px;
  }

  .keymap-chip {
    display: inline-flex;
    align-items: center;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-secondary);
  }

  .keymap-binding {
    font-family: "SF Mono", "Fira Code", "JetBrains Mono", monospace;
    font-size: 11px;
    padding: 3px 8px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text-primary);
    white-space: nowrap;
  }

  .keymap-chip .keymap-binding {
    border: none;
    padding-right: 4px;
  }

  .keymap-remove,
  .keymap-add {
    display: inline-grid;
    place-items: center;
    min-width: 24px;
    height: 22px;
    padding: 0 6px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--text-muted);
    font-size: 14px;
    line-height: 1;
    cursor: pointer;
  }

  .keymap-add {
    border: 1px dashed var(--border);
    color: var(--text-secondary);
  }

  .keymap-recorder {
    min-width: 7rem;
    height: 24px;
    padding: 0 10px;
    border: 1px solid var(--accent);
    border-radius: 4px;
    background: var(--bg-secondary);
    color: var(--text-primary);
    font-size: 12px;
    cursor: default;
  }

  .keymap-none,
  .keymap-fixed {
    font-size: 11px;
    color: var(--text-muted);
  }

  .keymap-notice,
  .keymap-conflict {
    grid-column: 1 / -1;
    margin: 0;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .keymap-notice.error {
    color: var(--accent-red, var(--error, #d1242f));
  }

  .keymap-conflict {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  @media (max-width: 640px) {
    /* Stack the description above its keys so neither is pushed off screen. */
    .keymap-row {
      grid-template-columns: 1fr;
    }

    .keymap-keys {
      justify-content: flex-start;
    }
  }
</style>
