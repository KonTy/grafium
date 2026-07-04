<script lang="ts">
  import {
    listFlashcardsDue,
    listFlashcardTopics,
    gradeFlashcard,
    importAnkiApkg,
    type Flashcard,
    type FlashcardTopic,
  } from "../lib/api";
  import { renderBlock, hydrateAssetMedia } from "../lib/markdown";
  import { open } from "@tauri-apps/plugin-dialog";
  import { listen } from "@tauri-apps/api/event";

  interface Props {
    onNavigate?: (title: string) => void;
  }

  let { onNavigate }: Props = $props();

  // "topics" = deck picker; "review" = an active study session.
  let view = $state<"topics" | "review">("topics");
  let topics = $state<FlashcardTopic[]>([]);
  let mixedDue = $state(0);
  // The topic being studied: null = mixed (all topics); "" = untagged; else a tag.
  let selectedTopic = $state<string | null>(null);

  let cards = $state<Flashcard[]>([]);
  let index = $state(0);
  let showBack = $state(false);
  let loading = $state(true);
  let reviewed = $state(0);
  let grading = $state(false);

  // Anki import state.
  let importing = $state(false);
  let importMsg = $state<string | null>(null);
  let importPhase = $state("");
  let importCurrent = $state(0);
  let importTotal = $state(0);

  // 0 total => indeterminate (scrolling) bar; otherwise a determinate percentage.
  const importIndeterminate = $derived(importTotal === 0);
  const importPct = $derived(
    importTotal > 0 ? Math.min(100, Math.round((importCurrent / importTotal) * 100)) : 0,
  );
  const importLabel = $derived(
    importPhase === "reading"
      ? "Reading deck…"
      : importPhase === "media"
        ? `Copying media ${importCurrent} / ${importTotal}`
        : importPhase === "indexing"
          ? `Indexing ${importTotal} cards…`
          : importPhase === "done"
            ? "Finishing up…"
            : "Importing…",
  );

  const current = $derived(cards[index] ?? null);

  // The rendered card container, used to hydrate <audio>/<video> media that
  // WebKitGTK can't load from the custom asset scheme.
  let cardEl = $state<HTMLElement | null>(null);

  // quality → label mapping for the SM-2 grades.
  const grades = [
    { q: 0, label: "Again", hint: "forgot", cls: "again" },
    { q: 3, label: "Hard", hint: "barely", cls: "hard" },
    { q: 4, label: "Good", hint: "recalled", cls: "good" },
    { q: 5, label: "Easy", hint: "instant", cls: "easy" },
  ];

  $effect(() => {
    loadTopics();
  });

  // Re-hydrate media whenever the visible card face changes.
  $effect(() => {
    // Track dependencies so this re-runs on card flip / navigation.
    void current;
    void showBack;
    const el = cardEl;
    // Wait for the {@html ...} DOM to update before querying it.
    queueMicrotask(() => hydrateAssetMedia(el));
  });

  function topicLabel(topic: string): string {
    return topic === "" ? "Untagged" : `#${topic}`;
  }

  async function loadTopics() {
    loading = true;
    try {
      const [t, due] = await Promise.all([
        listFlashcardTopics(),
        listFlashcardsDue(10000),
      ]);
      topics = t;
      mixedDue = due.length;
    } catch (e) {
      console.error("Failed to load topics", e);
      topics = [];
      mixedDue = 0;
    } finally {
      loading = false;
    }
  }

  async function study(topic: string | null) {
    selectedTopic = topic;
    view = "review";
    loading = true;
    try {
      cards = await listFlashcardsDue(100, topic ?? undefined);
      index = 0;
      showBack = false;
      reviewed = 0;
    } catch (e) {
      console.error("Failed to load flashcards", e);
      cards = [];
    } finally {
      loading = false;
    }
  }

  function backToTopics() {
    view = "topics";
    cards = [];
    loadTopics();
  }

  async function importAnki() {
    if (importing) return;
    importMsg = null;
    const selected = await open({
      multiple: false,
      title: "Import Anki deck",
      filters: [{ name: "Anki deck", extensions: ["apkg"] }],
    });
    if (!selected || typeof selected !== "string") return;
    importing = true;
    importPhase = "reading";
    importCurrent = 0;
    importTotal = 0;
    const unlisten = await listen<{ phase: string; current: number; total: number }>(
      "anki-import-progress",
      (e) => {
        importPhase = e.payload.phase;
        importCurrent = e.payload.current;
        importTotal = e.payload.total;
      },
    );
    try {
      const summary = await importAnkiApkg(selected);
      importMsg = `Imported “${summary.page_title}” — ${summary.card_count} cards (#${summary.topic})`;
      await loadTopics();
    } catch (e) {
      console.error("Anki import failed", e);
      importMsg = `Import failed: ${e}`;
    } finally {
      unlisten();
      importing = false;
    }
  }

  function reveal() {
    showBack = true;
  }

  async function grade(quality: number) {
    if (!current || grading) return;
    grading = true;
    try {
      await gradeFlashcard(current.id, quality);
      reviewed += 1;
      index += 1;
      showBack = false;
    } catch (e) {
      console.error("Failed to grade flashcard", e);
    } finally {
      grading = false;
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (view !== "review" || loading || !current) return;
    if (!showBack) {
      if (e.key === " " || e.key === "Enter") {
        e.preventDefault();
        reveal();
      }
      return;
    }
    if (e.key === "1") grade(0);
    else if (e.key === "2") grade(3);
    else if (e.key === "3") grade(4);
    else if (e.key === "4") grade(5);
    else if (e.key === " " || e.key === "Enter") {
      e.preventDefault();
      grade(4);
    }
  }
</script>

<svelte:window on:keydown={onKeydown} />

<div class="review">
  {#if importing}
    <div class="import-overlay">
      <div class="import-card">
        <div class="import-title">Importing Anki deck</div>
        <div class="import-bar" class:indeterminate={importIndeterminate}>
          <div class="import-fill" style={importIndeterminate ? "" : `width:${importPct}%`}></div>
        </div>
        <div class="import-status">{importLabel}</div>
        <div class="import-note">Large decks can take a moment — please keep the app open.</div>
      </div>
    </div>
  {/if}
  {#if view === "topics"}
    <header class="review-header">
      <h1>Flashcards</h1>
      <button class="import-btn" disabled={importing} onclick={importAnki}>
        {importing ? "Importing…" : "Import Anki deck"}
      </button>
    </header>

    {#if importMsg}
      <div class="import-msg">{importMsg}</div>
    {/if}

    {#if loading}
      <div class="state">Loading…</div>
    {:else if topics.length === 0}
      <div class="state empty">
        <p class="big">No flashcards yet</p>
        <p class="hint">
          Create a flashcard by writing <code>Question :: Answer</code> in any block.
          Add a tag like <code>#chinese</code> or <code>#physics</code> to sort it into
          a study topic. It becomes reviewable after the page is saved.
          You can also <strong>Import Anki deck</strong> (.apkg) above to bring in existing cards.
        </p>
      </div>
    {:else}
      <p class="picker-intro">Pick a topic to study, or review everything mixed together.</p>
      <div class="topic-list">
        <button class="topic mixed" disabled={mixedDue === 0} onclick={() => study(null)}>
          <span class="topic-name">Mixed <span class="topic-sub">all topics</span></span>
          <span class="topic-counts">
            <span class="due" class:zero={mixedDue === 0}>{mixedDue} due</span>
          </span>
        </button>
        {#each topics as t}
          <button class="topic" disabled={t.due === 0} onclick={() => study(t.topic)}>
            <span class="topic-name">{topicLabel(t.topic)}</span>
            <span class="topic-counts">
              <span class="due" class:zero={t.due === 0}>{t.due} due</span>
              <span class="total">/ {t.total}</span>
            </span>
          </button>
        {/each}
      </div>
    {/if}
  {:else}
    <header class="review-header">
      <button class="back" onclick={backToTopics}>← Topics</button>
      <h1>{selectedTopic === null ? "Mixed" : topicLabel(selectedTopic)}</h1>
      {#if !loading && cards.length > 0 && current}
        <span class="progress">{index + 1} / {cards.length}</span>
      {/if}
    </header>

    {#if loading}
      <div class="state">Loading…</div>
    {:else if cards.length === 0}
      <div class="state empty">
        <p class="big">All caught up 🎉</p>
        <p>No cards are due in this topic right now.</p>
        <button class="primary" onclick={backToTopics}>Back to topics</button>
      </div>
    {:else if !current}
      <div class="state empty">
        <p class="big">Session complete ✅</p>
        <p>You reviewed {reviewed} card{reviewed === 1 ? "" : "s"}.</p>
        <button class="primary" onclick={backToTopics}>Back to topics</button>
      </div>
    {:else}
      <div class="card" bind:this={cardEl}>
        <div class="face front">{@html renderBlock(current.front)}</div>
        {#if showBack}
          <hr />
          <div class="face back">{@html renderBlock(current.back)}</div>
        {/if}
      </div>

      <div class="actions">
        {#if !showBack}
          <button class="primary reveal" onclick={reveal}>Show answer <kbd>Space</kbd></button>
        {:else}
          <div class="grades">
            {#each grades as g}
              <button class="grade {g.cls}" disabled={grading} onclick={() => grade(g.q)}>
                <span class="grade-label">{g.label}</span>
                <span class="grade-hint">{g.hint}</span>
              </button>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  .review {
    max-width: 720px;
    margin: 0 auto;
    padding: 32px 24px 64px;
    display: flex;
    flex-direction: column;
    min-height: 100%;
  }
  .review-header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    margin-bottom: 24px;
  }
  .review-header h1 {
    font-size: 1.5rem;
    margin: 0;
  }
  .import-btn {
    background: transparent;
    color: var(--accent, #7dd3fc);
    border: 1px solid var(--border, #2b3852);
    border-radius: 8px;
    padding: 6px 14px;
    font-size: 0.85rem;
    cursor: pointer;
  }
  .import-btn:hover:not(:disabled) {
    border-color: var(--accent, #7dd3fc);
  }
  .import-btn:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .import-msg {
    background: var(--bg-alt, #1a2232);
    border: 1px solid var(--border, #2b3852);
    border-radius: 8px;
    padding: 10px 14px;
    margin-bottom: 16px;
    color: var(--text, #d7dee8);
    font-size: 0.9rem;
  }
  .import-overlay {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(6, 10, 18, 0.55);
    backdrop-filter: blur(2px);
  }
  .import-card {
    width: min(420px, 86vw);
    background: var(--bg-alt, #1a2232);
    border: 1px solid var(--border, #2b3852);
    border-radius: 14px;
    padding: 24px 26px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.4);
    text-align: center;
  }
  .import-title {
    font-size: 1.05rem;
    font-weight: 600;
    margin-bottom: 16px;
    color: var(--text, #d7dee8);
  }
  .import-bar {
    position: relative;
    height: 8px;
    border-radius: 6px;
    background: var(--border, #2b3852);
    overflow: hidden;
  }
  .import-fill {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 0;
    border-radius: 6px;
    background: var(--accent, #2563eb);
    transition: width 0.25s ease;
  }
  /* Indeterminate: a fixed-width chip scrolls across the track. */
  .import-bar.indeterminate .import-fill {
    width: 34%;
    animation: import-scroll 1.1s ease-in-out infinite;
    transition: none;
  }
  @keyframes import-scroll {
    0% { left: -34%; }
    100% { left: 100%; }
  }
  .import-status {
    margin-top: 14px;
    font-size: 0.9rem;
    color: var(--text, #d7dee8);
    font-variant-numeric: tabular-nums;
  }
  .import-note {
    margin-top: 6px;
    font-size: 0.78rem;
    color: var(--text-muted, #8a94a6);
  }
  .progress {
    color: var(--text-muted, #8a94a6);
    font-variant-numeric: tabular-nums;
  }
  .state {
    text-align: center;
    color: var(--text-muted, #8a94a6);
    margin-top: 15vh;
  }
  .state .big {
    font-size: 1.4rem;
    color: var(--text, #e5e7eb);
    margin-bottom: 8px;
  }
  .state .hint {
    margin-top: 20px;
    font-size: 0.9rem;
    line-height: 1.6;
  }
  code {
    background: var(--bg-alt, #1f2937);
    padding: 2px 6px;
    border-radius: 4px;
    font-size: 0.85em;
  }
  .card {
    background: var(--bg-alt, #1a2232);
    border: 1px solid var(--border, #2b3852);
    border-radius: 14px;
    padding: 32px;
    min-height: 220px;
    display: flex;
    flex-direction: column;
    justify-content: center;
    font-size: 1.25rem;
    line-height: 1.6;
  }
  .card hr {
    border: none;
    border-top: 1px solid var(--border, #2b3852);
    margin: 24px 0;
    width: 100%;
  }
  .face.back {
    color: var(--accent, #7dd3fc);
  }
  .face :global(.fc-img) {
    max-width: 100%;
    max-height: 320px;
    height: auto;
    border-radius: 8px;
    margin: 8px auto;
    display: block;
  }
  .face :global(.fc-audio) {
    width: 100%;
    max-width: 320px;
    height: 40px;
    margin: 10px auto;
    display: block;
  }
  .face :global(.fc-video) {
    max-width: 100%;
    max-height: 320px;
    border-radius: 8px;
    margin: 8px auto;
    display: block;
  }
  .actions {
    margin-top: 28px;
    display: flex;
    justify-content: center;
  }
  button.primary {
    background: var(--accent, #2563eb);
    color: #fff;
    border: none;
    border-radius: 10px;
    padding: 14px 28px;
    font-size: 1rem;
    cursor: pointer;
  }
  button.primary:hover {
    filter: brightness(1.1);
  }
  .reveal kbd {
    background: rgba(255, 255, 255, 0.2);
    border-radius: 4px;
    padding: 1px 6px;
    margin-left: 8px;
    font-size: 0.8em;
  }
  .grades {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 12px;
    width: 100%;
  }
  .grade {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    padding: 14px 8px;
    border-radius: 10px;
    border: 1px solid var(--border, #2b3852);
    background: var(--bg-alt, #1a2232);
    color: var(--text, #e5e7eb);
    cursor: pointer;
  }
  .grade:hover:not(:disabled) {
    filter: brightness(1.15);
  }
  .grade:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .grade-label {
    font-weight: 600;
  }
  .grade-hint {
    font-size: 0.75rem;
    color: var(--text-muted, #8a94a6);
  }
  .grade.again {
    border-color: #ef4444;
  }
  .grade.hard {
    border-color: #f59e0b;
  }
  .grade.good {
    border-color: #22c55e;
  }
  .grade.easy {
    border-color: #38bdf8;
  }

  /* Topic picker */
  .back {
    background: none;
    border: none;
    color: var(--text-muted, #8a94a6);
    cursor: pointer;
    font-size: 0.9rem;
    padding: 0;
  }
  .back:hover {
    color: var(--text, #e5e7eb);
  }
  .picker-intro {
    color: var(--text-muted, #8a94a6);
    margin: 0 0 16px;
    font-size: 0.95rem;
  }
  .topic-list {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .topic {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-radius: 12px;
    border: 1px solid var(--border, #2b3852);
    background: var(--bg-alt, #1a2232);
    color: var(--text, #e5e7eb);
    cursor: pointer;
    text-align: left;
    font-size: 1.05rem;
  }
  .topic:hover:not(:disabled) {
    filter: brightness(1.15);
    border-color: var(--accent, #2563eb);
  }
  .topic:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .topic.mixed {
    border-color: var(--accent, #2563eb);
    background: linear-gradient(var(--bg-alt, #1a2232), var(--bg-alt, #1a2232)) padding-box,
      linear-gradient(90deg, #2563eb, #38bdf8) border-box;
    border: 1px solid transparent;
  }
  .topic-name {
    font-weight: 600;
  }
  .topic-sub {
    font-weight: 400;
    color: var(--text-muted, #8a94a6);
    font-size: 0.85rem;
    margin-left: 6px;
  }
  .topic-counts {
    font-variant-numeric: tabular-nums;
    font-size: 0.9rem;
  }
  .topic-counts .due {
    color: var(--accent, #7dd3fc);
    font-weight: 600;
  }
  .topic-counts .due.zero {
    color: var(--text-muted, #8a94a6);
    font-weight: 400;
  }
  .topic-counts .total {
    color: var(--text-muted, #8a94a6);
    margin-left: 4px;
  }
</style>
