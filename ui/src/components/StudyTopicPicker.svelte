<script lang="ts">
  import { autofocus } from "../lib/autofocus";

  let { value = $bindable("General"), topics, disabled = false }: {
    value?: string; topics: string[]; disabled?: boolean;
  } = $props();
  let creating = $state(false);
  const choices = $derived([...new Set(["General", ...topics, ...(!creating && value ? [value] : [])])]
    .filter(topic => topic.trim()).sort((a, b) => a.localeCompare(b)));
</script>

<div class="topic-picker">
  <label>Topic
    <select aria-label="Topic" value={creating ? "new" : `topic:${value || "General"}`} {disabled} onchange={event => {
      const selected = event.currentTarget.value;
      creating = selected === "new";
      value = creating ? "" : selected.slice(6);
    }}>
      {#each choices as topic}<option value={`topic:${topic}`}>{topic}</option>{/each}
      <option value="new">+ Add a new topic...</option>
    </select>
  </label>
  {#if creating}
    <label>New topic
      <input bind:value required {disabled} placeholder="Name your topic" maxlength="512" use:autofocus />
    </label>
  {/if}
</div>

<style>
  .topic-picker { display: flex; flex-direction: column; gap: 8px; min-width: 0; width: 100%; }
  label { display: flex; flex-direction: column; gap: 6px; font-size: 12px; color: var(--text-secondary); }
  input, select { min-width: 0; width: 100%; box-sizing: border-box; padding: 8px 10px; font: inherit; color: var(--text-primary); background: var(--bg-primary); border: 1px solid var(--border); border-radius: 7px; }
  input:focus-visible, select:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  :disabled { opacity: .55; }
</style>
