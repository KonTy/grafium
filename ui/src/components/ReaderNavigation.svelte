<script lang="ts">
  import { bionicReaderEnabled, setBionicReaderEnabled } from "../lib/bionicReader";
  import { readerFlow, setReaderFlow } from "../lib/readerPreferences";
  import type { ReaderTurn } from "../lib/readerNavigation";
  import { shortcutTitle } from "../lib/shortcuts";

  let { ready, reflowable, direction = "ltr", onNavigate }: {
    ready: boolean; reflowable: boolean; direction?: "ltr" | "rtl";
    onNavigate: (direction: ReaderTurn) => void;
  } = $props();
</script>

<button class="page-turn" aria-label="Previous page" title="Previous page (Page Up)"
  aria-keyshortcuts={direction === "rtl" ? "ArrowRight PageUp" : "ArrowLeft PageUp"}
  disabled={!ready} onclick={() => onNavigate("prev")}>
  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
    <path d={direction === "rtl" ? "m9 5 7 7-7 7" : "m15 5-7 7 7 7"} />
  </svg>
</button>
<button class="page-turn" aria-label="Next page" title="Next page (Page Down)"
  aria-keyshortcuts={direction === "rtl" ? "ArrowLeft PageDown" : "ArrowRight PageDown"}
  disabled={!ready} onclick={() => onNavigate("next")}>
  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
    <path d={direction === "rtl" ? "m15 5-7 7 7 7" : "m9 5 7 7-7 7"} />
  </svg>
</button>
{#if reflowable}
  <select aria-label="Reading layout" title="Reading layout" value={$readerFlow} disabled={!ready}
    onchange={event => {
      const value = event.currentTarget.value;
      if (value === "paginated" || value === "scrolled") setReaderFlow(value);
    }}>
    <option value="paginated">Pages</option>
    <option value="scrolled">Continuous scroll</option>
  </select>
{/if}
<button class="page-turn" aria-label="Bionic reading"
  title={!ready ? "Bionic reading is available after the book opens" : !reflowable
    ? "Bionic reading is unavailable for PDF or fixed-layout pages" : shortcutTitle("Bionic reading", "toggle-bionic")}
  aria-pressed={reflowable && $bionicReaderEnabled} disabled={!ready || !reflowable}
  onclick={() => setBionicReaderEnabled(!$bionicReaderEnabled)}>
  <span><strong>B</strong>ionic</span>
</button>

<style>
  button, select { font: inherit; font-size: 12px; color: var(--text-primary); background: var(--bg-primary); border: 1px solid var(--border); border-radius: 6px; min-height: 44px; padding: 7px; }
  button { cursor: pointer; }
  .page-turn { display: inline-grid; place-items: center; min-width: 44px; }
  button[aria-pressed="true"] { color: var(--accent); border-color: var(--accent); }
  button:disabled, select:disabled { opacity: .5; cursor: default; }
  button:focus-visible, select:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
