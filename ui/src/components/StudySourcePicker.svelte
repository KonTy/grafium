<script lang="ts">
  import { untrack } from "svelte";
  import { getPage, listAssets, listFlashcardTopics, listPageSummaries, type Page } from "../lib/api";
  import { autofocus } from "../lib/autofocus";
  import { isOriginalBookPage } from "../lib/books";
  import { fetchStudyLinkTitle, type StudyItem } from "../lib/studies";
  import { isStudyLinkInput, isStudySourceAdded, searchStudyCatalog, studyCatalog, type StudyCandidate, type StudySourceChoice } from "../lib/studyCatalog";
  import { normalizeStudySource, studyKindLabels, studySourceFromLink } from "../lib/studySources";

  let { graphPath, items, initialPage = null, choice = $bindable(null), disabled = false }: {
    graphPath: string; items: StudyItem[]; initialPage?: Page | null;
    choice?: StudySourceChoice | null; disabled?: boolean;
  } = $props();
  const listId = $props.id();
  let query = $state("");
  let candidates = $state<StudyCandidate[]>([]);
  let loading = $state(false);
  let loadError = $state("");
  let selectionError = $state("");
  let titleError = $state("");
  let titleLoading = $state(false);
  let choosing = $state(false);
  let expanded = $state(true);
  let detailsOpen = $state(false);
  let activeKey = $state("");
  let visibleCount = $state(40);
  let searchInput: HTMLInputElement;
  let titleEdited = false;
  let generation = 0;
  let selectionRequest = 0;
  let titleTimer: ReturnType<typeof setTimeout> | undefined;
  const matches = $derived(isStudyLinkInput(query) ? [] : searchStudyCatalog(candidates, query));
  const visible = $derived(matches.slice(0, visibleCount));
  const activeIndex = $derived(visible.findIndex(candidate => candidate.key === activeKey));
  const showResults = $derived(expanded && !isStudyLinkInput(query));

  function cancelSelection() {
    clearTimeout(titleTimer); ++selectionRequest;
    titleLoading = false; choosing = false; titleError = ""; selectionError = "";
  }

  async function loadSources() {
    const current = ++generation;
    const graph = graphPath;
    loading = true; loadError = "";
    const groups: StudyCandidate[][] = [[], [], []];
    const errors = ["", "", ""];
    let remaining = 3;
    const currentGraph = () => current === generation && graph === graphPath;
    async function load<T>(index: number, label: string, request: Promise<T>, convert: (result: T) => StudyCandidate[]) {
      try {
        const result = await request;
        if (currentGraph()) groups[index] = convert(result);
      } catch (cause) {
        if (currentGraph()) errors[index] = `Could not load ${label}: ${String(cause)}`;
      } finally {
        if (currentGraph()) {
          candidates = groups.flat(); loadError = errors.filter(Boolean).join(" ");
          loading = --remaining > 0;
        }
      }
    }
    await Promise.all([
      load(0, "pages and books", listPageSummaries(), pages => studyCatalog(pages, [], [])),
      load(1, "flashcard topics", listFlashcardTopics(), cards => studyCatalog([], cards, [])),
      load(2, "media assets", listAssets(), assets => studyCatalog([], [], assets)),
    ]);
  }

  $effect(() => {
    graphPath;
    untrack(() => {
      cancelSelection(); choice = null; query = ""; candidates = []; expanded = true; detailsOpen = false; titleEdited = false;
      void loadSources();
    });
    return () => { ++generation; cancelSelection(); };
  });

  $effect(() => {
    const page = initialPage;
    if (page) untrack(() => {
      cancelSelection(); titleEdited = false; query = page.title; expanded = false;
      choice = { kind: isOriginalBookPage(page) ? "book" : "page", source: page.id, title: page.title };
    });
  });

  function search(value: string) {
    const editedTitle = titleEdited ? choice?.title : undefined;
    cancelSelection(); choice = null; query = value; activeKey = ""; visibleCount = 40;
    expanded = true;
    if (!isStudyLinkInput(value)) { titleEdited = false; detailsOpen = false; return; }
    expanded = false;
    try {
      const detected = studySourceFromLink(value);
      choice = { kind: detected.kind, source: detected.source, title: editedTitle ?? detected.filenameTitle ?? "" };
      if (detected.filenameTitle !== undefined) return;
      titleLoading = true;
      const request = selectionRequest;
      const graph = graphPath;
      titleTimer = setTimeout(() => {
        void fetchStudyLinkTitle(detected.source).then(title => {
          if (request === selectionRequest && graph === graphPath && choice && !titleEdited) choice = { ...choice, title };
        }).catch(cause => {
          if (request === selectionRequest && graph === graphPath) {
            titleError = `Could not fetch the title: ${String(cause)}. Enter a title below or retry.`;
            detailsOpen = true;
          }
        }).finally(() => { if (request === selectionRequest) titleLoading = false; });
      }, 450);
    } catch (cause) { selectionError = String(cause); }
  }

  function alreadyAdded(candidate: StudySourceChoice) {
    return isStudySourceAdded(items, candidate);
  }

  async function choose(candidate: StudyCandidate) {
    if (alreadyAdded(candidate)) return;
    cancelSelection(); choice = null; titleEdited = false; detailsOpen = false;
    query = candidate.title; expanded = false; activeKey = "";
    searchInput?.focus({ preventScroll: true });
    if (candidate.kind !== "page" && candidate.kind !== "book") {
      choice = { kind: candidate.kind, source: candidate.source, title: candidate.title };
      return;
    }
    const request = selectionRequest;
    const graph = graphPath;
    choosing = true;
    try {
      const page = await getPage({ id: candidate.source });
      if (request !== selectionRequest || graph !== graphPath) return;
      choice = { kind: isOriginalBookPage(page) ? "book" : "page", source: page.id, title: page.title };
      query = page.title;
    } catch (cause) {
      if (request === selectionRequest && graph === graphPath) {
        selectionError = `Could not open source: ${String(cause)}`; expanded = true;
      }
    } finally { if (request === selectionRequest) choosing = false; }
  }

  function keydown(event: KeyboardEvent) {
    if (event.isComposing) return;
    if (event.key === "Escape" && expanded) {
      event.preventDefault(); event.stopPropagation(); expanded = false;
    } else if (["ArrowDown", "ArrowUp"].includes(event.key) && !isStudyLinkInput(query)) {
      event.preventDefault(); event.stopPropagation(); expanded = true;
      const indexes = visible.flatMap((candidate, index) => alreadyAdded(candidate) ? [] : [index]);
      const next = (event.key === "ArrowDown" ? indexes.find(index => index > activeIndex) ?? indexes[0]
        : indexes.slice().reverse().find(index => index < activeIndex) ?? indexes.at(-1)) ?? -1;
      activeKey = visible[next]?.key ?? "";
      document.getElementById(`${listId}-${next}`)?.scrollIntoView?.({ block: "nearest" });
    } else if (event.key === "Enter" && showResults) {
      event.preventDefault(); event.stopPropagation();
      const candidate = activeIndex >= 0 ? visible[activeIndex] : visible.find(candidate => !alreadyAdded(candidate));
      if (candidate) void choose(candidate);
    }
  }

  function changeKind(kind: StudyItem["kind"]) {
    if (!choice) return;
    cancelSelection();
    try { choice = { ...choice, kind, source: normalizeStudySource(kind, choice.source) }; }
    catch (cause) { selectionError = String(cause); }
  }
</script>

<div class="source-picker">
  <label>Search or paste a link
    <input bind:this={searchInput} value={query} oninput={event => search(event.currentTarget.value)}
      onkeydown={keydown} onfocus={() => { if (!choice) expanded = true; }}
      role="combobox" aria-expanded={showResults} aria-controls={listId} aria-autocomplete="list"
      aria-activedescendant={showResults && activeIndex >= 0 ? `${listId}-${activeIndex}` : undefined}
      autocomplete="off" placeholder="Pages, books, flashcard topics, videos, MP3s, or a URL"
      {disabled} use:autofocus />
  </label>
  {#if showResults}
    <ul id={listId} role="listbox" aria-label="Study sources">
      {#each visible as candidate, index (candidate.key)}
        <li role="presentation"><button type="button" role="option" id={`${listId}-${index}`}
          aria-selected={activeIndex === index} class:active={activeIndex === index}
          aria-label={`${candidate.title} — ${studyKindLabels[candidate.kind]}${alreadyAdded(candidate) ? " — Already added" : ""}`}
          disabled={disabled || alreadyAdded(candidate)} tabindex="-1" onclick={() => choose(candidate)}>
          <span class="badge">{studyKindLabels[candidate.kind]}</span>
          <span class="result-text"><strong>{candidate.title}</strong><small>{candidate.detail}</small></span>
          {#if alreadyAdded(candidate)}<small>Already added</small>{/if}
        </button></li>
      {/each}
    </ul>
    {#if matches.length > visibleCount}<button type="button" {disabled} onclick={() => visibleCount += 40}>Show more results ({matches.length - visibleCount})</button>{/if}
    {#if !matches.length && !loading}<p>No matching sources. Try another name or paste a web link.</p>{/if}
    <small>Arrow keys browse; Enter selects. Nothing is added until you press Add study.</small>
  {/if}
  {#if loading}<p role="status">Loading sources...</p>{/if}
  {#if loadError}<p class="error" role="alert">{loadError} <button type="button" {disabled} onclick={loadSources}>Retry sources</button></p>{/if}
  {#if choosing}<p role="status">Selecting source...</p>{/if}
  {#if titleLoading}<p role="status">Fetching title...</p>{/if}
  {#if selectionError}<p class="error" role="alert">{selectionError}</p>{/if}
  {#if titleError}<p class="error" role="status">{titleError} <button type="button" {disabled} onclick={() => search(query)}>Retry title lookup</button></p>{/if}
  {#if choice}
    <div class="selected-source" aria-label="Selected source">
      <span class="badge">{studyKindLabels[choice.kind]}</span>
      <div><strong>{choice.title || "Title needed"}</strong><small>{choice.kind === "flashcards" ? `${choice.source ? "#" + choice.source : "Untagged"} flashcards` : choice.source}</small></div>
    </div>
    {#if choice.kind === "website"}<small>Opens in your browser with a manual checkpoint. Browser time is not tracked.</small>{/if}
    {#if alreadyAdded(choice)}<p role="status">This source is already in Studies. Open its existing entry instead.</p>{/if}
    <details bind:open={detailsOpen}>
      <summary>Edit details</summary>
      <label>Title<input value={choice.title} required {disabled} oninput={event => {
        titleEdited = true; if (choice) choice = { ...choice, title: event.currentTarget.value };
      }} /></label>
      {#if ["website", "youtube", "audio", "video"].includes(choice.kind)}
        <label>Source type<select aria-label="Source type" value={choice.kind} {disabled}
          onchange={event => {
            const kind = event.currentTarget.value;
            if (kind === "website" || kind === "youtube" || kind === "audio" || kind === "video") changeKind(kind);
            event.currentTarget.value = choice?.kind ?? "website";
          }}>
          {#each (["website", "youtube", "audio", "video"] as const) as kind}<option value={kind}>{studyKindLabels[kind]}</option>{/each}
        </select></label>
        <small>Only change the type for unusual links, such as an extensionless audio/video stream.</small>
      {/if}
    </details>
  {/if}
  <small>Graph search stays local. Pasted links contact YouTube or the website to fetch a title; media is not downloaded.</small>
</div>

<style>
  .source-picker { display: flex; flex-direction: column; gap: 10px; min-width: 0; }
  label { display: flex; flex-direction: column; gap: 6px; font-size: 12px; color: var(--text-secondary); }
  input, select, button { font: inherit; color: var(--text-primary); border: 1px solid var(--border); border-radius: 7px; background: var(--bg-primary); padding: 8px 10px; min-width: 0; }
  input:focus-visible, button:focus-visible, select:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  ul { list-style: none; padding: 4px; margin: 0; max-height: 240px; overflow: auto; border: 1px solid var(--border); border-radius: 8px; }
  li button { display: flex; align-items: center; gap: 12px; width: 100%; border: 0; text-align: left; cursor: pointer; }
  li button:hover, li button.active { background: var(--bg-hover, var(--bg-secondary)); }
  .result-text { display: flex; flex: 1; flex-direction: column; min-width: 0; gap: 4px; }
  strong { font-size: 13px; overflow-wrap: anywhere; }
  small { font-size: 11px; color: var(--text-muted); overflow-wrap: anywhere; }
  .badge { color: var(--accent); font-size: 11px; flex-shrink: 0; min-width: 60px; }
  .selected-source { display: flex; align-items: center; gap: 12px; padding: 12px; border: 1px solid var(--border); border-radius: 8px; }
  .selected-source div { display: flex; flex-direction: column; gap: 5px; min-width: 0; }
  details label { margin: 10px 0; } summary { cursor: pointer; font-size: 12px; color: var(--text-secondary); }
  p { margin: 0; font-size: 12px; } .error { color: var(--accent-red, #e78284); overflow-wrap: anywhere; }
  button:disabled, input:disabled, select:disabled { opacity: .55; cursor: default; }
</style>
