<script lang="ts">
  import type { Page } from "../lib/api";
  import { bookAnnotationTarget } from "../lib/books";
  import type { PageNavigationTarget } from "../lib/navigation";
  import BookNotesPanel from "./BookNotesPanel.svelte";

  let { page, onNavigate }: { page: Page; onNavigate: (target: PageNavigationTarget) => void } = $props();
  const target = $derived(bookAnnotationTarget(page));
</script>

<section class="annotation-editor" aria-label="Book annotation editor" data-help-context="books">
  {#if target}
    <button class="return" onclick={() => onNavigate({ id: target.bookId })}>Return to book</button>
    <BookNotesPanel pageId={target.bookId} pageTitle={page.title} focusNoteId={target.noteId} {onNavigate} />
  {:else}
    <p role="alert">This annotation's book reference is unavailable. The annotation file has not been changed.</p>
  {/if}
</section>

<style>
  .annotation-editor { display:flex; flex-direction:column; flex:1; min-width:0; min-height:0; padding:16px; gap:12px; }
  .return { align-self:flex-start; font:inherit; padding:6px 10px; border:1px solid var(--border); border-radius:5px; background:var(--bg-tertiary); color:var(--text-primary); cursor:pointer; }
  .return:focus-visible { outline:2px solid var(--accent); outline-offset:2px; }
</style>
