<script lang="ts">
  import { listFlashcardsDue, gradeFlashcard, type Flashcard } from "../lib/api";
  import { renderBlock } from "../lib/markdown";

  interface Props {
    onNavigate?: (title: string) => void;
  }

  let { onNavigate }: Props = $props();

  let cards = $state<Flashcard[]>([]);
  let index = $state(0);
  let showBack = $state(false);
  let loading = $state(true);
  let reviewed = $state(0);
  let grading = $state(false);

  const current = $derived(cards[index] ?? null);

  // quality → label mapping for the SM-2 grades.
  const grades = [
    { q: 0, label: "Again", hint: "forgot", cls: "again" },
    { q: 3, label: "Hard", hint: "barely", cls: "hard" },
    { q: 4, label: "Good", hint: "recalled", cls: "good" },
    { q: 5, label: "Easy", hint: "instant", cls: "easy" },
  ];

  $effect(() => {
    load();
  });

  async function load() {
    loading = true;
    try {
      cards = await listFlashcardsDue(100);
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
    if (loading || !current) return;
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
  <header class="review-header">
    <h1>Flashcard Review</h1>
    {#if !loading && cards.length > 0 && current}
      <span class="progress">{index + 1} / {cards.length}</span>
    {/if}
  </header>

  {#if loading}
    <div class="state">Loading…</div>
  {:else if cards.length === 0}
    <div class="state empty">
      <p class="big">All caught up 🎉</p>
      <p>No cards are due for review right now.</p>
      <p class="hint">
        Create a flashcard by writing <code>Question :: Answer</code> in any block
        (optionally tag it <code>#flashcard</code>). It becomes reviewable after the
        page is saved.
      </p>
    </div>
  {:else if !current}
    <div class="state empty">
      <p class="big">Session complete ✅</p>
      <p>You reviewed {reviewed} card{reviewed === 1 ? "" : "s"}.</p>
      <button class="primary" onclick={load}>Review again</button>
    </div>
  {:else}
    <div class="card">
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
</style>
