<script lang="ts">
  import { workflowPreviewLine, type WorkflowProposal } from "../lib/assistantWorkflows";
  import { splitIntoBlocks } from "../lib/aiActions";
  import { isTaskContent } from "../lib/taskSyntax";
  let { proposal, applying = false, error = "", onApply, onDismiss, onEdit }: {
    proposal: WorkflowProposal; applying?: boolean; error?: string;
    onApply: () => void; onDismiss: () => void;
    onEdit: (update: Partial<Pick<WorkflowProposal, "title" | "content">>) => void;
  } = $props();
  let editing = $state(false);
  const containsTasks = $derived(isTaskContent(proposal.title) || splitIntoBlocks(proposal.content).some(isTaskContent));
</script>

<section class="workflow-card" aria-label="Review ASK action" data-help-context="chat">
  <header><strong>Proposed changes</strong><span>Nothing is saved until you choose Apply.</span></header>
  <p class="coverage">{proposal.summary}</p>
  {#if proposal.snapshot.kind === "rewrite"}
    <p>Append below the original on <strong>{proposal.snapshot.sourcePageTitle}</strong>. Original notes stay unchanged.</p>
  {:else}
    <p>Create a reference page in this graph. {proposal.snapshot.kind === "tasks" ? "Original tasks, their order, dates, and states stay unchanged. Possible duplicates are suggestions, not deletions." : "Source notes stay unchanged."}</p>
  {/if}
  <label>{proposal.snapshot.kind === "rewrite" ? "Draft heading" : "New page title"}
    <input aria-label={proposal.snapshot.kind === "rewrite" ? "Draft heading" : "New page title"} value={proposal.title}
      oninput={event => onEdit({ title: event.currentTarget.value })} disabled={applying} />
  </label>
  <button class="quiet" aria-pressed={editing} disabled={applying} onclick={() => { editing = !editing; }}>
    {editing ? "Preview draft" : "Edit draft"}
  </button>
  {#if editing}
    <label>Draft text<textarea value={proposal.content} oninput={event => onEdit({ content: event.currentTarget.value })} disabled={applying} rows="14"></textarea></label>
  {:else}
    <div class="preview" aria-label="Proposed draft">
      {#each proposal.content.split("\n") as line}
        <div class:heading={/^#{1,6} /.test(line)}>
          {#each workflowPreviewLine(line, proposal.snapshot) as part}
            {#if part.source}
              <button class="source" title={`Open source on ${part.source.title}`} disabled={applying}
                onclick={() => window.dispatchEvent(new CustomEvent("navigate-page", {
                  detail: { pageName: part.source!.title, targetBlockId: part.source!.blockId },
                }))}>{part.text}</button>
            {:else}{part.text}{/if}
          {/each}
        </div>
      {/each}
    </div>
  {/if}
  {#if containsTasks}<p>Task markers in this draft will be quoted so it creates no extra tasks.</p>{/if}
  {#if error}<p role="alert">{error}</p>{/if}
  <footer>
    <button class="apply" disabled={applying || !proposal.title.trim() || !proposal.content.trim()} onclick={onApply}>{applying ? "Applying…" : "Apply"}</button>
    <button class="quiet" disabled={applying} onclick={onDismiss}>Dismiss</button>
  </footer>
</section>

<style>
  .workflow-card { border: 1px solid var(--accent); border-radius: 8px; padding: 12px; display: grid; gap: 10px; min-width: 0; }
  header, label { display: grid; gap: 5px; }
  header span, .coverage { color: var(--text-secondary); font-size: 12px; }
  p { margin: 0; overflow-wrap: anywhere; }
  input, textarea, button { font: inherit; color: var(--text-primary); }
  input, textarea { box-sizing: border-box; width: 100%; min-width: 0; background: var(--bg-primary); border: 1px solid var(--border); padding: 7px; border-radius: 5px; }
  textarea { resize: vertical; }
  button { cursor: pointer; padding: 6px 10px; border-radius: 5px; border: 1px solid var(--border); background: var(--bg-secondary); }
  .quiet { justify-self: start; }
  .apply { background: var(--accent); color: var(--bg-primary); }
  .preview { max-height: 480px; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; line-height: 1.6; }
  .preview > div { min-height: 1em; }
  .heading { font-weight: 600; margin-top: 8px; }
  .source { border: 0; padding: 0; background: transparent; color: var(--accent); text-align: left; white-space: pre-wrap; }
  footer { display: flex; gap: 8px; }
  [role="alert"] { color: var(--danger, #c44); }
  :is(button, input, textarea):focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  button:disabled { opacity: .55; cursor: default; }
</style>
