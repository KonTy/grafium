<script lang="ts">
  import { untrack } from "svelte";
  import type { Page } from "../lib/api";
  import { listStudies, saveStudy, removeStudy, type StudyItem, type StudyDay } from "../lib/studies";
  import { normalizeStudySource, studyKindLabels, studyPercent, studyTime, localStudyDay } from "../lib/studySources";
  import { isStudySourceAdded, type StudySourceChoice } from "../lib/studyCatalog";
  import StudyTopicPicker from "./StudyTopicPicker.svelte";
  import StudySourcePicker from "./StudySourcePicker.svelte";

  let { graphPath, onOpen, addPage = null }: {
    graphPath: string; onOpen: (item: StudyItem) => void; addPage?: Page | null;
  } = $props();
  let items = $state<StudyItem[]>([]);
  let days = $state<StudyDay[]>([]);
  let topicHistory = $state<string[]>([]);
  let loading = $state(true);
  let error = $state("");
  let formError = $state("");
  let topicFilter = $state("");
  let libraryQuery = $state("");
  let formOpen = $state(false);
  let choice = $state<StudySourceChoice | null>(null);
  let sourcePage = $state<Page | null>(null);
  let topic = $state("General");
  let saving = $state(false);
  let editId = $state("");
  let editTopic = $state("");
  let removeId = $state("");
  let pendingId = $state("");
  let generation = 0;
  const topics = $derived([...new Set(items.map(item => item.topic || "General"))].sort((a, b) => a.localeCompare(b)));
  const topicChoices = $derived([...new Set([...topicHistory, ...topics])]);
  const filtered = $derived(items.filter(item => (!topicFilter || (item.topic || "General") === topicFilter.slice(6))
    && `${item.title} ${item.topic || "General"} ${studyKindLabels[item.kind]}`.toLocaleLowerCase().includes(libraryQuery.trim().toLocaleLowerCase())));
  const visibleIds = $derived(new Set(filtered.map(item => item.id)));
  const visibleDays = $derived(days.filter(day => visibleIds.has(day.itemId)));
  const totalTime = $derived(visibleDays.reduce((sum, day) => sum + day.seconds, 0));
  const todayTime = $derived(visibleDays.filter(day => day.day === localStudyDay()).reduce((sum, day) => sum + day.seconds, 0));
  const duplicate = $derived(choice ? isStudySourceAdded(items, choice) : false);

  $effect(() => {
    const graph = graphPath;
    const current = ++generation;
    items = []; days = []; topicHistory = []; loading = true; error = ""; formOpen = false;
    topicFilter = ""; libraryQuery = ""; editId = ""; removeId = ""; pendingId = ""; saving = false; choice = null; sourcePage = null;
    void listStudies(graph).then(snapshot => {
      if (current !== generation) return;
      items = snapshot.items; days = snapshot.days; topicHistory = snapshot.topics ?? [];
    }).catch(cause => {
      if (current === generation) error = `Could not load studies: ${String(cause)}`;
    }).finally(() => { if (current === generation) loading = false; });
    return () => { ++generation; };
  });

  $effect(() => {
    graphPath;
    const page = addPage;
    if (page) untrack(() => {
      sourcePage = page; formOpen = true; topic = "General"; formError = "";
    });
  });

  function startAdd() {
    sourcePage = null; choice = null; formOpen = true;
    topic = topicFilter ? topicFilter.slice(6) : "General"; formError = "";
  }

  async function addStudy(event: SubmitEvent) {
    event.preventDefault();
    const graph = graphPath;
    const current = generation;
    formError = "";
    try {
      if (!choice) throw new Error("Choose a source for this study.");
      const { kind, source, title } = choice;
      const normalized = normalizeStudySource(kind, source);
      if (!title.trim()) throw new Error("Give this study a title.");
      if (!topic.trim()) throw new Error("Give the topic a name or choose an existing topic.");
      if (duplicate) throw new Error("This source is already in Studies. Open its existing entry instead.");
      saving = true;
      const now = new Date().toISOString();
      const saved = await saveStudy(graph, {
        id: crypto.randomUUID(), title: title.trim(), topic: topic.trim() || "General", kind, source: normalized,
        progress: { position: 0, total: 0, anchor: "", label: "" }, createdAt: now, updatedAt: now,
      });
      if (current !== generation || graph !== graphPath) return;
      items = [...items, saved]; topicHistory = [...new Set([...topicHistory, saved.topic])]; formOpen = false;
    } catch (cause) {
      if (current === generation && graph === graphPath) formError = String(cause);
    } finally { if (current === generation) saving = false; }
  }

  async function updateTopic(item: StudyItem) {
    const graph = graphPath; const current = generation;
    pendingId = item.id; error = "";
    try {
      if (!editTopic.trim()) throw new Error("Give the topic a name or choose an existing topic.");
      const saved = await saveStudy(graph, { ...item, topic: editTopic.trim() || "General" });
      if (current !== generation || graph !== graphPath) return;
      topicHistory = [...new Set([...topicHistory, saved.topic])];
      items = items.map(entry => entry.id === saved.id ? saved : entry); editId = "";
    } catch (cause) { if (current === generation) error = `Could not save topic: ${String(cause)}`; }
    finally { if (current === generation) pendingId = ""; }
  }

  async function confirmRemove(item: StudyItem) {
    const graph = graphPath; const current = generation;
    pendingId = item.id; error = "";
    try {
      await removeStudy(graph, item.id);
      if (current !== generation || graph !== graphPath) return;
      items = items.filter(entry => entry.id !== item.id); removeId = "";
    } catch (cause) { if (current === generation) error = `Could not remove study: ${String(cause)}`; }
    finally { if (current === generation) pendingId = ""; }
  }

  function itemTime(id: string) { return studyTime(days.filter(day => day.itemId === id).reduce((sum, day) => sum + day.seconds, 0)); }
</script>

<section class="studies" data-help-context="studies">
  <header><div><p class="eyebrow">YOUR LEARNING LIBRARY</p><h1>Studies</h1><p class="subtitle">Choose a source. Keep your place. Make time to learn.</p></div><button class="primary" disabled={saving || formOpen} onclick={startAdd}>+ Add study</button></header>
  <div class="stats" aria-label="Study statistics">
    <div><span>Total study time</span><strong>{studyTime(totalTime)}</strong><small>{libraryQuery.trim() ? "Filtered studies" : topicFilter ? "Selected topic" : "All studies"}</small></div>
    <div><span>Today</span><strong>{studyTime(todayTime)}</strong><small>Active study time</small></div>
    <div><span>In your library</span><strong>{filtered.length}</strong><small>{filtered.length === 1 ? "Study" : "Studies"}</small></div>
  </div>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if formOpen}
    <form class="add-form" onsubmit={addStudy}>
      <h2>Add to Studies</h2>
      <StudySourcePicker {graphPath} {items} initialPage={sourcePage} bind:choice disabled={saving} />
      <StudyTopicPicker bind:value={topic} topics={topicChoices} disabled={saving} />
      {#if formError}<p class="error" role="alert">{formError}</p>{/if}
      <div class="actions"><button class="primary" type="submit" disabled={saving || loading || !choice?.title.trim() || !!duplicate}>{saving ? "Saving…" : "Add study"}</button><button type="button" disabled={saving} onclick={() => formOpen = false}>Cancel</button></div>
    </form>
  {/if}
  <div class="library-heading"><h2>Your studies</h2><div class="library-controls"><input type="search" data-local-search aria-label="Filter studies" bind:value={libraryQuery} placeholder="Filter studies…" /><label>Topic<select bind:value={topicFilter}><option value="">All topics</option>{#each topics as value}<option value={`topic:${value}`}>{value}</option>{/each}</select></label></div></div>
  {#if loading}<p role="status" class="empty">Loading your studies…</p>
  {:else if !filtered.length}<div class="empty"><h3>{items.length ? "No matching studies" : "A little learning, every day"}</h3><p>{items.length ? "Try another topic or search term." : "Add a page, a book, flashcards, or a media source to build your own study list."}</p></div>
  {:else}
    <div class="column-headings" aria-hidden="true"><span>Type</span><span>Study / progress</span><span>Topic</span><span class="time-heading">Time</span><span></span></div>
    <ul class="study-list">
      {#each filtered as item (item.id)}
        <li class="study-row">
          <div class="kind" aria-label={studyKindLabels[item.kind]}>{studyKindLabels[item.kind]}</div>
          <div class="details">
            <button class="study-title" onclick={() => onOpen(item)}>{item.title}</button>
            <div class="progress-line"><progress max="100" value={studyPercent(item.progress)} aria-label={`${item.title} progress`}></progress><span>{item.progress.label || (item.progress.total > 0 ? `${studyPercent(item.progress)}%` : item.kind === "website" ? "Manual checkpoint" : "Not started")}</span></div>
          </div>
          <div class="topic-cell">
            <span class="topic-label">Topic</span>
            {#if editId === item.id}
              <form class="topic-edit" onsubmit={(event) => { event.preventDefault(); void updateTopic(item); }}><StudyTopicPicker bind:value={editTopic} topics={topicChoices} disabled={pendingId === item.id} /><button type="submit" disabled={pendingId === item.id}>Save topic</button><button type="button" onclick={() => editId = ""}>Cancel</button></form>
            {:else}
              <button class="topic" title="Edit topic" onclick={() => { editId = item.id; editTopic = item.topic || "General"; }}>{item.topic || "General"} <span aria-hidden="true">✎</span></button>
            {/if}
          </div>
          <div class="row-time"><strong>{itemTime(item.id)}</strong><small>studied</small></div>
          <div class="row-actions"><button class="open" onclick={() => onOpen(item)}>{item.progress.position > 0 || item.progress.label ? "Continue" : "Open"}</button><button class="remove" aria-label={`Remove ${item.title} from Studies`} onclick={() => removeId = item.id}>×</button></div>
          {#if removeId === item.id}<div class="remove-confirm" role="group" aria-label="Confirm removal"><p>Remove “{item.title}” from Studies? Only this study entry and its study history are removed. Your source page, book, cards, or media will not be deleted.</p><button disabled={pendingId === item.id} onclick={() => confirmRemove(item)}>Remove study entry</button><button disabled={pendingId === item.id} onclick={() => removeId = ""}>Keep study</button></div>{/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .studies { container-type: inline-size; max-width: 1120px; margin: 0 auto; padding: 32px clamp(16px, 4vw, 48px); color: var(--text-primary); }
  header, .library-heading, .actions, .row-actions { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
  h1 { font-size: 30px; margin: 4px 0 8px; letter-spacing: -.025em; } h2 { font-size: 17px; margin: 0; } h3 { margin: 0 0 8px; }
  .eyebrow { font-size: 10px; font-weight: 700; letter-spacing: .15em; color: var(--accent); margin: 0; }
  .subtitle, small, .empty p { color: var(--text-muted); } .subtitle { margin: 0; font-size: 13px; }
  button, input, select { font: inherit; color: var(--text-primary); border: 1px solid var(--border); border-radius: 7px; background: var(--bg-primary); padding: 8px 10px; }
  button { cursor: pointer; } button:hover { background: var(--bg-hover, var(--bg-secondary)); } button:disabled { opacity: .55; cursor: default; }
  button:focus-visible, input:focus-visible, select:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  .primary, .open { color: var(--accent); border-color: color-mix(in srgb, var(--accent) 45%, var(--border)); background: color-mix(in srgb, var(--accent) 8%, var(--bg-primary)); white-space: nowrap; }
  .stats { display: grid; grid-template-columns: repeat(3, 1fr); gap: 14px; margin: 28px 0; } .stats > div { padding: 18px 20px; background: var(--bg-secondary); border: 1px solid var(--border); border-radius: 12px; display: flex; flex-direction: column; gap: 6px; }
  .stats span { color: var(--text-secondary); font-size: 12px; } .stats strong { font-size: 26px; font-weight: 600; } small { font-size: 11px; }
  .add-form { padding: 20px; border: 1px solid var(--border); border-radius: 12px; margin-bottom: 24px; background: var(--bg-secondary); display: flex; flex-direction: column; gap: 12px; }
  label { display: flex; flex-direction: column; gap: 6px; font-size: 12px; color: var(--text-secondary); } input { min-width: 0; }
  .actions { justify-content: flex-start; }
  .library-heading { margin: 20px 0 14px; } .library-heading label { flex-direction: row; align-items: center; }
  .library-controls { display: flex; align-items: center; justify-content: flex-end; gap: 10px; flex-wrap: wrap; } .library-controls input { width: 180px; font-size: 12px; }
  .study-list { list-style: none; margin: 0; padding: 0; border: 1px solid var(--border); border-radius: 12px; overflow: hidden; }
  .study-row, .column-headings { display: grid; grid-template-columns: 76px minmax(130px, 1fr) minmax(100px, .6fr) 65px 110px; align-items: center; gap: 18px; padding: 20px; }
  .column-headings { padding-top: 0; padding-bottom: 9px; color: var(--text-muted); font-size: 11px; } .time-heading { text-align: right; }
  .study-row { border-bottom: 1px solid var(--border); } .study-row:last-child { border-bottom: none; }
  .kind { font-size: 11px; font-weight: 600; padding: 12px 6px; text-align: center; color: var(--accent); background: color-mix(in srgb, var(--accent) 8%, var(--bg-secondary)); border-radius: 8px; }
  .details, .topic-cell { min-width: 0; } .topic-label { display: none; } .study-title { display: block; text-align: left; padding: 0; border: 0; background: none; font-weight: 600; font-size: 15px; overflow-wrap: anywhere; }
  .topic { border: 0; padding: 4px 0; color: var(--text-secondary); background: none; font-size: 12px; text-align: left; overflow-wrap: anywhere; } .topic span { opacity: .6; }
  .progress-line { display: flex; align-items: center; flex-wrap: wrap; gap: 10px; margin-top: 10px; color: var(--text-muted); font-size: 11px; } .progress-line span { overflow-wrap: anywhere; }
  progress { width: 110px; height: 5px; border: 0; border-radius: 8px; background: var(--border); accent-color: var(--accent); flex-shrink: 0; } progress::-webkit-progress-bar { background: var(--border); border-radius: 8px; } progress::-webkit-progress-value { background: var(--accent); border-radius: 8px; } progress::-moz-progress-bar { background: var(--accent); border-radius: 8px; }
  .row-time { display: flex; flex-direction: column; align-items: flex-end; gap: 4px; } .row-time strong { font-size: 13px; font-weight: 500; white-space: nowrap; } .remove { color: var(--text-muted); border: 0; background: none; font-size: 20px; }
  .topic-edit { display: flex; align-items: end; gap: 6px; margin: 8px 0; flex-wrap: wrap; } .topic-edit button { font-size: 11px; }
  .empty { padding: 48px 24px; text-align: center; border: 1px dashed var(--border); border-radius: 12px; color: var(--text-secondary); } .error { color: var(--accent-red, #e78284); overflow-wrap: anywhere; }
  .remove-confirm { grid-column: 1 / -1; font-size: 12px; padding: 12px; background: var(--bg-secondary); border-radius: 8px; } .remove-confirm p { margin: 0 0 10px; } .remove-confirm button { margin-right: 8px; }
  @container (max-width: 720px) { .column-headings { display: none; } .study-row { grid-template-columns: 66px minmax(0, 1fr) auto; gap: 12px; padding: 14px; } .details { grid-column: 2 / 4; } .topic-cell { grid-column: 2; } .topic-label { display: block; color: var(--text-muted); font-size: 10px; } .row-actions { grid-column: 3; grid-row: 2; gap: 4px; } .row-time { grid-column: 1; grid-row: 2; align-items: center; } }
  @media (max-width: 650px) { header { align-items: flex-start; } .stats { gap: 8px; } .stats > div { padding: 12px; } .stats strong { font-size: 21px; } .library-heading { align-items: flex-start; } }
</style>
