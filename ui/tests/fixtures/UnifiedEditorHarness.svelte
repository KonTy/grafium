<script>
  import UnifiedPageEditor from "../../src/components/UnifiedPageEditor.svelte";
  import ReferencePanel from "../../src/components/ReferencePanel.svelte";
  import { getPage } from "../../src/lib/api";
  import { onMount } from "svelte";
  import { registerPageEditorReload } from "../../src/lib/editorPersistence";
  import { bionicReaderEnabled, loadBionicReaderPreference, setBionicReaderEnabled } from "../../src/lib/bionicReader";

  let page = $state(window.__selectionState.pages[0]);
  let visible = $state(false);
  let noteFocusLabel = $state(null);
  let noteFocusPageId = $state(null);
  let noteFocusTrigger = $state(0);
  let navigationError = $state("");
  let editor;
  $effect(() => registerPageEditorReload(page.id, async () => {
    if (!editor) throw new Error("The isolated source editor is not mounted.");
    await editor.reloadSource();
  }));
  async function navigate(target) {
    const lookup = typeof target === "string" ? { title: target }
      : target.pageId ? { id: target.pageId } : target.pageName ? { title: target.pageName } : target;
    try {
      const next = await getPage(lookup);
      if (next.id !== page.id) page = next;
    } catch (error) { navigationError = String(error); }
  }
  onMount(() => {
    loadBionicReaderPreference();
    const toggle = () => { visible = !visible; };
    const openNote = ({ detail }) => {
      noteFocusLabel = detail.footnoteLabel;
      noteFocusPageId = detail.pageId;
      noteFocusTrigger++;
      visible = true;
    };
    const navigateEvent = ({ detail }) => { void navigate(detail); };
    window.addEventListener("toggle-reference-panel", toggle);
    window.addEventListener("open-reading-note", openNote);
    window.addEventListener("navigate-page", navigateEvent);
    return () => {
      window.removeEventListener("toggle-reference-panel", toggle);
      window.removeEventListener("open-reading-note", openNote);
      window.removeEventListener("navigate-page", navigateEvent);
    };
  });
</script>

<div class="fixture-shell">
  <main class="main-content">
    <p class="fixture-banner">Isolated retained component. The application's continuous-editor feature remains disabled.</p>
    <button title="Bionic Speedreader" aria-pressed={$bionicReaderEnabled}
      onclick={() => setBionicReaderEnabled(!$bionicReaderEnabled)}>Bionic Speedreader</button>
    <h1>{page.title}</h1>
    {#if navigationError}<p role="alert">{navigationError}</p>{/if}
    <div class="page-content"><UnifiedPageEditor bind:this={editor} {page} /></div>
  </main>
  <ReferencePanel {visible} pageId={page.id} pageTitle={page.title} initialTab="notes"
    {noteFocusLabel} {noteFocusPageId} {noteFocusTrigger} onClose={() => visible = false} onNavigate={navigate} />
</div>

<style>
  .fixture-shell { display:flex; height:100vh; color:var(--text-primary); background:var(--bg-primary); }
  .main-content { flex:1; min-width:0; overflow:auto; padding:24px; }
  .fixture-banner { font-size:12px; color:var(--text-secondary); }
</style>
