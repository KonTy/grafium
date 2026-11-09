<script lang="ts">
  import { answerIqItem, finishIqSession, startIqSession, type IqPackSummary, type IqResult, type IqSession } from "../lib/iq";
  let { pack, onExit }: { pack: IqPackSummary; onExit: () => void } = $props();
  let age = $state<number | undefined>(undefined);
  let language = $state("");
  let unseen = $state(false);
  let accepted = $state(false);
  let session = $state<IqSession | null>(null);
  let result = $state<IqResult | null>(null);
  let index = $state(0);
  let answer = $state("");
  let busy = $state(false);
  let error = $state("");
  let now = $state(Date.now());
  let finishRequested = $state(false);
  const remaining = $derived(session?.expiresAt ? Math.max(0, Math.ceil((session.expiresAt - now) / 1000)) : null);
  const canStart = $derived(accepted && (!pack.norms || (age !== undefined && Number.isInteger(age) && age >= 0 && age <= 120 && language.trim() !== "")));

  $effect(() => {
    if (!session || result) return;
    const timer = setInterval(() => { now = Date.now(); }, 250);
    return () => clearInterval(timer);
  });
  $effect(() => {
    if (session && !result && remaining === 0 && !busy && !finishRequested) {
      finishRequested = true;
      void finish();
    }
  });

  async function start() {
    if (!canStart || busy) return;
    busy = true;
    error = "";
    try {
      session = await startIqSession(pack.id, { age: age ?? 0, language, unseen });
      now = Date.now();
    } catch (cause) {
      console.error("Could not start IQ assessment", cause);
      error = `Could not start assessment: ${String(cause)}`;
    } finally { busy = false; }
  }
  async function finish() {
    if (!session || busy) return;
    busy = true;
    error = "";
    try {
      result = await finishIqSession(session);
    } catch (cause) {
      console.error("Could not finish IQ assessment", cause);
      error = `Could not save assessment result: ${String(cause)}`;
    } finally { busy = false; }
  }
  async function submit(skip = false) {
    if (!session || busy || (remaining === 0)) return;
    busy = true;
    error = "";
    try {
      await answerIqItem(session, index, skip ? "" : answer);
      answer = "";
      index += 1;
    } catch (cause) {
      console.error("Could not save IQ answer", cause);
      error = `Could not save answer: ${String(cause)}`;
    } finally { busy = false; }
    if (index === session.questions.length) await finish();
  }
</script>

<section data-help-context="iq" aria-label="IQ-marked pack assessment">
  <h1>{pack.title}</h1>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if result}
    <h2>Reasoning score: {result.correct} / {result.total} ({result.percentage.toFixed(1)}%)</h2>
    <p>{result.unanswered} unanswered · {result.elapsedSeconds} seconds</p>
    {#if result.estimatedIq !== null}
      <h2>Approximate IQ: {Math.round(result.estimatedIq)}</h2>
      <p role="note">Based on unverified, pack-supplied population norms using IQ = 100 + 15z.
        This is not a clinical assessment or evidence of increased intelligence.</p>
      {#if result.normCitation}<a href={result.normCitation} target="_blank" rel="noopener noreferrer">Norm source</a>{/if}
    {:else}
      <p role="note">{result.iqUnavailableReason}</p>
    {/if}
    <details>
      <summary>Answers and explanations</summary>
      {#each result.items as item, i}
        <article>
          <h3>{i + 1}. {item.correct ? "Correct" : "Incorrect or skipped"}</h3>
          <pre>{item.question}</pre>
          <p>Your answer: {item.answer || "(unanswered)"}</p>
          <p>Accepted: {item.acceptedAnswers.join(" / ")}</p>
          <pre>{item.explanation}</pre>
        </article>
      {/each}
    </details>
    <button onclick={onExit}>Back to packs</button>
  {:else if !session}
    <p>{pack.itemCount} questions · Answers appear only after completion · No change to repetition schedules</p>
    {#if pack.norms}
      <p role="note">An approximate IQ uses this pack's unverified norms, not a clinical assessment.
        Outside the norm group or after prior practice, only a reasoning score is reported.</p>
      <p>Norm population: {pack.norms.population}; sample: {pack.norms.sampleSize};
        ages {pack.norms.ageMin}–{pack.norms.ageMax}; language: {pack.norms.language}.</p>
      <a href={pack.norms.citation} target="_blank" rel="noopener noreferrer">Read norm source</a>
      <pre>{pack.norms.instructions}</pre>
      <p>{pack.norms.timeLimitSeconds ? `${pack.norms.timeLimitSeconds} seconds, no pauses` : "Untimed"}</p>
      <label>Age <input type="number" min="0" max="120" step="1" bind:value={age} /></label>
      <label>Test language code <input bind:value={language} placeholder={pack.norms.language} /></label>
      <label><input type="checkbox" bind:checked={unseen} /> I have not previously seen or practised these questions, including outside Grafium.</label>
      <label><input type="checkbox" bind:checked={accepted} /> I will follow the stated instructions without aids or assistance, and understand these norms are not independently verified.</label>
    {:else}
      <p role="note">This pack has no population norms. It reports a reasoning score, not IQ.</p>
      <label><input type="checkbox" bind:checked={accepted} /> I understand this is reasoning practice, not an IQ measurement.</label>
    {/if}
    <button disabled={!canStart || busy} onclick={start}>{busy ? "Starting…" : "Start assessment"}</button>
    <button disabled={busy} onclick={onExit}>Back to packs</button>
  {:else}
    <p>Question {Math.min(index + 1, session.questions.length)} / {session.questions.length}
      {#if remaining !== null} · {remaining} seconds remaining{/if}</p>
    {#if index < session.questions.length && remaining !== 0}
      <pre class="question">{session.questions[index]}</pre>
      <form onsubmit={(event) => { event.preventDefault(); void submit(); }}>
        <label>Your answer <textarea bind:value={answer} disabled={busy} maxlength="8192" rows="3"></textarea></label>
        <button type="submit" disabled={busy || !answer.trim()}>Submit answer</button>
        <button type="button" disabled={busy} onclick={() => { void submit(true); }}>Skip</button>
      </form>
      <button disabled={busy} onclick={() => { void finish(); }}>Finish early (reasoning score only)</button>
    {:else}
      <p>{remaining === 0 ? "Time is up. Unanswered questions score zero." : "All answers submitted."}</p>
      <button disabled={busy} onclick={() => { void finish(); }}>{busy ? "Saving result…" : "Save result"}</button>
    {/if}
  {/if}
</section>

<style>
  section { display: flex; flex-direction: column; gap: 12px; }
  label { display: block; }
  textarea { display: block; width: 100%; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; font: inherit; }
  .question { font-family: monospace; font-size: 1.1em; }
  article { padding: 12px 0; border-bottom: 1px solid var(--border-color, #888); }
  button { align-self: flex-start; }
</style>
