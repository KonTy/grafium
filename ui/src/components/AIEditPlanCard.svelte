<script lang="ts">
  /**
   * Review card for a proposed {@link EditPlan}.
   *
   * The whole safety story of AI editing rests on this component: the model
   * proposes, the user disposes. Everything is editable here — heading, tags and
   * body — so a nearly-right plan is a quick fix rather than a re-prompt, and
   * the graph is only touched when Apply is clicked.
   */
  import { actionUsesContent, describeAction, normalizeTag, type EditAction } from "../lib/aiActions";

  let {
    request = "",
    actions = [],
    applying = false,
    error = "",
    onApply,
    onDismiss,
  }: {
    request?: string;
    actions?: EditAction[];
    applying?: boolean;
    error?: string;
    onApply: () => void;
    onDismiss: () => void;
  } = $props();

  function setTags(action: EditAction, raw: string) {
    action.tags = raw
      .split(",")
      .map((tag) => normalizeTag(tag))
      .filter(Boolean);
  }

  // Enough rows to read the note without turning the panel into a scroll maze.
  function rowsFor(content: string): number {
    return Math.min(14, Math.max(3, content.split("\n").length + 1));
  }
</script>

<div class="edit-plan" role="group" aria-label="Proposed changes to your notes">
  <header class="plan-header">
    <strong>Proposed changes</strong>
    <span>Nothing is saved until you press Apply. Edit anything below first.</span>
  </header>

  {#if request}
    <p class="plan-request">You asked: “{request}”</p>
  {/if}

  {#each actions as action (action)}
    <div class="plan-action">
      <p class="plan-what">{describeAction(action)}</p>
      {#if actionUsesContent(action.type)}
        <label class="plan-field">
          <span>Heading</span>
          <input type="text" bind:value={action.title} disabled={applying} placeholder="Short label for this note" />
        </label>
        <label class="plan-field">
          <span>Tags</span>
          <input
            type="text"
            value={action.tags.join(", ")}
            disabled={applying}
            placeholder="health/supplements, longevity"
            oninput={(event) => setTags(action, event.currentTarget.value)}
          />
        </label>
        <label class="plan-field">
          <span>Text</span>
          <textarea bind:value={action.content} disabled={applying} rows={rowsFor(action.content)}></textarea>
        </label>
      {/if}
    </div>
  {/each}

  {#if error}<p class="error-message" role="alert">{error}</p>{/if}

  <footer class="plan-footer">
    <button class="apply-button" onclick={onApply} disabled={applying || !actions.length}>
      {applying ? "Applying…" : "Apply"}
    </button>
    <button class="quiet-button" onclick={onDismiss} disabled={applying}>Dismiss</button>
  </footer>
</div>

<style>
  .edit-plan {
    border: 1px solid var(--accent, #6b8afd);
    border-radius: 8px;
    padding: 10px 12px;
    margin: 8px 0;
    background: var(--bg-secondary, rgba(107, 138, 253, 0.06));
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .plan-header {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .plan-header span {
    font-size: 0.8em;
    opacity: 0.75;
  }
  .plan-request {
    margin: 0;
    font-size: 0.85em;
    opacity: 0.8;
  }
  .plan-action {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px;
    border-radius: 6px;
    background: var(--bg-primary, rgba(0, 0, 0, 0.12));
  }
  .plan-what {
    margin: 0;
    font-weight: 600;
    font-size: 0.9em;
  }
  .plan-field {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 0.78em;
    opacity: 0.9;
  }
  .plan-field input,
  .plan-field textarea {
    font: inherit;
    font-size: 1.05em;
    padding: 4px 6px;
    border-radius: 4px;
    border: 1px solid var(--border, rgba(128, 128, 128, 0.4));
    background: var(--bg-input, transparent);
    color: inherit;
    width: 100%;
    box-sizing: border-box;
  }
  .plan-field textarea {
    resize: vertical;
    min-height: 3em;
  }
  .plan-footer {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .apply-button {
    font: inherit;
    padding: 4px 14px;
    border-radius: 4px;
    border: 1px solid var(--accent, #6b8afd);
    background: var(--accent, #6b8afd);
    color: #fff;
    cursor: pointer;
  }
  .apply-button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .error-message {
    margin: 0;
    color: var(--error, #e5534b);
    font-size: 0.85em;
  }
</style>
